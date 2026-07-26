<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0180 — own the scoring graph by `Arc`, and retire the `&'static` escape hatch

Status: implemented
Implemented in: 2026-07-26
App: prototext-graph, protolens
Refs: docs/scoring-flaws.md (C9),
      docs/protolens/rendering-worklist.md (W4, W8),
      docs/protolens/rendering-flaws.md (C3, P4, A5),
      docs/specs/0152-protolens-background-scoring-worker.md (G2, G9),
      docs/specs/0172-score-all-veto-correctness-and-graph-load-validation.md (S4)

## Background

`LoadedGraph` (`prototext-graph/src/score/load.rs:21`) holds an mmap and
a reference into it:

```rust
pub struct LoadedGraph {
    _backing: GraphBacking,
    /// Zero-copy view into the backing storage.
    pub graph: &'static ArchivedCompiledGraph,
}
```

The `'static` is manufactured by `std::mem::transmute` at `:144`, under a
safety comment that states the invariant precisely:

> `payload` borrows `mmap`, which `LoadedGraph` keeps alive for exactly as
> long as `graph`.

**The struct does not keep that promise, because the field is `pub` and
`&'static T` is `Copy`.** Any caller can copy the reference out and outlive
the `LoadedGraph`; the compiler will not object, because as far as the type
system is concerned the reference really is `'static`. The invariant is
enforced by every caller remembering it, which is not enforcement.

### This is not hypothetical — it is already load-bearing in two places

**One place remembered, at a cost.** `App` (`protolens/src/tui/mod.rs:738`)
carries a 12-line comment explaining that `heat_worker` must be *declared*
before `ctx`, because Rust drops fields in declaration order and
`HeatWorkerHandle::drop` joins a thread that is still using the graph. The
comment records that this was "a use-after-unmap race hit intermittently at
process exit (2026-07-25 bug report)". A struct whose field *order* is a
safety invariant is a structure that will be broken by the next person who
sorts its fields.

**One place did not remember.** The root-type inference thread
(`mod.rs:1680`) is spawned detached and never joined:

```rust
let graph_ref = graph.graph; // 'static, Copy — spec 0152 G2
...
thread::spawn(move || {
    let fqdn = decode::resolve_root_winner_fqdn(&original_blob, graph_ref);
    let _ = root_type_tx.send(event::AppEvent::RootTypeResolved(fqdn));
});
```

with this justification:

> Not joined anywhere (deliberately, unlike `heat_worker`/`input_reader`
> below): it holds only `'static`/`Arc`-owned data, so an early
> `should_quit` simply lets the OS reclaim it mid-sweep.

**`graph_ref` is not `'static`-owned data. It is a `transmute`.** When
`run()` returns, `App` drops, `ctx` drops, `LoadedGraph` drops, and the
`Mmap` is unmapped — while this thread may still be in the middle of a
`score_all` sweep over the whole blob, which is precisely the longest-running
scoring operation in the program. The result is a read of unmapped memory:
`SIGBUS` on the mmap path, or a use-after-free of the leaked `AlignedVec` on
the `from_static_bytes` path.

The field-ordering fix does **not** cover this thread, because there is no
handle to drop. So the same defect that was found and patched once is still
live in the sibling call site fifteen lines above the patch.

### Why the fix is `Arc` and not a join

Joining the detached thread would work, and it is the wrong trade: it makes
quitting wait out the very sweep that was moved off the critical path to
avoid blocking the user. The comment above is right about *what it wants* —
fire and forget — and wrong only about whether the data it holds supports
that. Giving the thread an owning handle makes the comment true as written.

This is also what makes the ordering hazard go away rather than move: with an
`Arc`, `App`'s field order stops being load-bearing, the worker cannot outlive
the mapping, and the compiler is checking it rather than a comment.

## Goals

- **G1** — `LoadedGraph`'s `'static` reference is not reachable from outside
  `prototext-graph`. The `transmute` stays (it is how zero-copy mmap access
  works), but it becomes a genuinely private implementation detail, so its
  safety argument is discharged by the module rather than asserted to callers.
- **G2** — the detached root-type thread and the heat worker each hold an
  owning handle to the graph, so neither can observe an unmapped page,
  whatever order anything drops in and whether or not either is joined.
- **G3** — `App`'s field-order comment is downgraded from an invariant to a
  historical note, in place, rather than deleted.

## Non-goals

- **N1** — making `blob` an `Arc<Vec<u8>>` (W8's second half, flaws P4/A5,
  the 3×→1× blob memory win). It is a memory optimization with no soundness
  content, it touches the override-splicing paths that mutate the blob, and
  bundling it would make a safety fix hard to review. Left to W8's remainder.
- **N2** — changing `load_graph`'s return type to `Arc<LoadedGraph>`, as W8
  proposes. `prototext`'s CLI is single-threaded and holds the graph in its
  own `DescriptorContext`; forcing an `Arc` on it buys nothing and costs an
  allocation and an indirection. The soundness property comes from privacy
  (G1), not from `Arc`. `Arc` is applied where a reference must outlive a
  borrow — which is protolens, and only protolens.
- **N3** — any change to the scoring walk, the graph format, or `VERSION`.
- **N4** — auditing the *other* `unsafe` in `load.rs` (`access_unchecked` on
  the `from_static_bytes` path). It is a separate question with a separate
  answer, and spec 0172 S4 already covers the header validation that guards
  it.

## Specification

### S1. `LoadedGraph::graph` becomes private, with an accessor

```rust
pub struct LoadedGraph {
    _backing: GraphBacking,
    graph: &'static ArchivedCompiledGraph,
}

impl LoadedGraph {
    /// The archived graph, borrowed for as long as `self` — which is the
    /// lifetime the backing mmap actually has.
    pub fn graph(&self) -> &ArchivedCompiledGraph { self.graph }
}
```

The existing `Deref` impl is kept: it is the ergonomic path, and it already
returns the correctly-shortened lifetime. The accessor exists because
`g.graph()` reads better than `&**g` at a call site that needs an explicit
`&ArchivedCompiledGraph`.

Every reader of the field outside `load.rs` becomes `g.graph()`. That is the
whole of the change in `prototext`, `prototext-graph`'s tests, and
`benches/score.rs`: those call sites want a borrow, and always did.

The safety comment at the `transmute` gains one sentence recording that
privacy is what discharges it, so a future reader who makes the field `pub`
again is told what they are breaking.

### S2. protolens owns the graph by `Arc`

- `DescriptorContext.graph` (`protolens/src/decode.rs:59`) becomes
  `Option<Arc<LoadedGraph>>`.
- `HeatWorkerHandle::spawn` (`heat_worker.rs:435`) and `heat_worker_loop`
  (`:348`) take `Arc<LoadedGraph>` instead of `&'static ArchivedCompiledGraph`,
  and call `.graph()` at the point of use inside the loop.
- The root-type thread (`mod.rs:1680`) captures a cloned `Arc` and calls
  `.graph()` inside the closure.

`LoadedGraph` is `Send + Sync` (its `Mmap` is, and `ArchivedCompiledGraph` is
plain archived data), so `Arc<LoadedGraph>` is `Send` and the two spawns need
no other change.

Call sites that read `ctx.graph` and then mutate `self` may need the `Arc`
cloned first rather than borrowed through `self.ctx` — the `&'static` copy
used to detach the borrow for free, and it no longer does. Cloning an `Arc`
is the correct fix there and is a refcount bump, not a copy of the graph.

### S3. The comments that recorded the hazard are updated, not deleted

- `mod.rs:727-737` (`App`'s field-order note) keeps its history and states
  that the ordering is **no longer load-bearing**, with the reason.
- `mod.rs:1663-1672` (the detached thread's note) keeps its argument for not
  joining, and its "holds only `'static`/`Arc`-owned data" claim becomes true:
  the `Arc` is now named as what makes it true.
- `load.rs`'s safety comment names privacy as the discharging property.

### S4. The root-type thread gets the scoring stack size

The same spawn site carries a second defect (rendering-flaws C3(b)): it runs
`resolve_root_winner_fqdn` → `score_all` — the deepest possible input, the
whole blob — on a bare `thread::spawn` with `std`'s 2 MiB default, while the
heat worker running the identical code gets 16 MiB from
`HEAT_WORKER_STACK_SIZE`.

This is not speculative and it is not urgent, which is exactly why it should
be closed now rather than argued about later. `MAX_WIRE_DEPTH`'s doc comment
(`prototext-core/src/helpers/bounds.rs`) records the measurement: the scorer
consumes ~576 KiB for a full-cap nest in release, so this thread's margin is
**3.6× — the binding margin in the entire workspace**, and every other
(walker, thread) pair has more room. It is also the one pair that goes
negative in a debug build.

The constant becomes `SCORING_THREAD_STACK_SIZE` in `tui/mod.rs`, the parent
of both spawn sites, and both name it. The report is explicit that the reason
the second spawn silently missed it is that it lived in `heat_worker.rs`,
reachable from one of the two places that needs it. A stack reservation costs
address space, not resident pages, so widening the smaller of the two to match
is free in the only currency that is scarce.

## Test plan

- **The compiler is the proving test**, per W8: after S1 no
  `&'static ArchivedCompiledGraph` is obtainable outside `prototext-graph`, so
  the old pattern must fail to compile. This is a negative property and is
  asserted by construction rather than by a test case.
- **The existing suites must stay green unchanged** — `prototext-graph`'s 70
  scoring tests, protolens's TUI suites, and the `reproto` pytest suite.
  Nothing observable changes; if a test changes, something else did too.
- **Worker lifecycle** — `heat_worker.rs`'s three worker tests already spawn a
  real thread against a real in-memory graph. They exercise the new ownership
  directly once they hold an `Arc`, and their `test_scoring_graph()` helper
  stops needing the `&'static` rebinding line.
- **No new test asserts the absence of a race.** A use-after-unmap at process
  exit is timing-dependent and a passing test would be evidence of nothing.
  The guarantee here is structural, and the honest place to record it is the
  type, which is what S1/S2 change.
