<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0168 — protolens: resolve the root type before decoding, not after

Status: draft
App: protolens

## Background

Startup currently decodes the document twice.

`decode()` takes a `defer_root_type` flag (`decode.rs:733-752`). When
set — which is the default whenever there is a scoring graph and no
explicit `--type` — it skips `determine_root_type` entirely, leaving
`root_desc` as `None`. The whole document is therefore decoded and
rendered as **raw / no type**: `decode_and_render_indexed` walks every
byte, `colorize` parses the entire rendered text, `build_tree` builds the
full arena.

Only then does `run()` spawn a detached thread (`mod.rs:1673-1684`) to
run `resolve_root_winner_fqdn` over the same blob. When it reports back,
`AppEvent::RootTypeResolved` (`mod.rs:1844-1849`) calls
`apply_resolved_root_type`, which installs a root override and runs
`render_overrides` from the root — splicing the root node, i.e.
**re-decoding and re-rendering the entire document** under the newly
discovered type, and paying a full `finalize_override_batch` on top.

Measured against a 1.1 MB `FileDescriptorSet` (193,072 lines / 622,922
nodes, `--release`; see `docs/protolens/rendering-scaling-roadmap.md`):

| Step | Cost |
|---|---|
| `decode()` | 1.65 s |
| `App::new` | 5.22 s |
| a root-node override commit (what `apply_resolved_root_type` performs) | 10.6 s |

So the raw first pass is not a cheap placeholder that gets refined — it
is thrown away wholesale and repeated at greater cost, because the
second pass goes through the splice machinery rather than the decode
path. The user sees a fully rendered document, browses it, and then has
it replaced under them several seconds later, with every line and every
node index changed.

The asynchrony was introduced (in the unnumbered changes commented
`spec NNNN` throughout `decode.rs`/`mod.rs` — no such spec file exists;
see Non-goals N5) to fix a real problem: a multi-second black screen at
startup while the sweep ran. But asynchrony fixed the *symptom*
(nothing on screen) by adding a second full render, when the symptom's
actual cause was the absence of a progress indication during a blocking
phase that startup already has — `decode()` itself blocks for 1.65 s and
always has.

### The sweep is also computed twice

`resolve_root_winner_fqdn` (`decode.rs:188-199`) and
`override_pane::inferred_candidates` (`override_pane.rs:56-73`) are the
same computation: same `score_all`, same `ScoringOpts::default()`, same
sort comparator. They differ only in the tail — the resolver requires a
unique non-vetoed winner and returns `Option<String>`, the pane returns
the whole non-vetoed list.

They also run on the same bytes. The resolver is passed
`blob[wrapper_offset..]` (`mod.rs:1679`), which is exactly the root
node's interior payload range — the value `heat_cue_for` would use as a
`by_range` key for the root node.

The resolver writes nothing into `HeatCaches`, so the root range is
scored a second time the first time the heat cue or the override pane
needs it.

### Two safety defects fall out of the same site

Documented as C3 in `docs/protolens/rendering-flaws.md`:

- The detached thread copies out `graph.graph`, a `&'static
  ArchivedCompiledGraph` fabricated by a lifetime extension over an
  `Mmap` owned by `App.ctx` (`prototext-graph/src/score/load.rs:82-89`).
  It is never joined, and neither of the two mechanisms protecting the
  heat worker (explicit `shutdown()`; `heat_worker`-before-`ctx` field
  order) covers it. Quitting mid-sweep unmaps the file under a running
  thread.
- It runs `score_all`'s deep recursion on `thread::spawn`'s 2 MiB
  default stack — the exact hazard `HEAT_WORKER_STACK_SIZE = 16 MiB`
  exists to avoid for the heat worker.

Both disappear if the sweep runs on the main thread before the TUI
starts.

## Goals

- **G1**: Decode the document exactly once at startup, under the root
  type that will actually be used — eliminating the raw-then-retyped
  double pass and the root-node override commit that performs it.
- **G2**: Remove the detached root-type thread, closing
  `rendering-flaws.md` C3 (a) and (b).
- **G3**: Seed the root range's scoring result into `HeatCaches` so the
  heat cue and the override pane reuse it rather than re-scoring the
  same bytes.
- **G4**: Keep startup legible: the terminal must show what protolens is
  doing during the sweep, rather than going blank. This is what the
  asynchrony was actually buying and it must not regress.
- **G5**: Leave the winner rule itself byte-identical — same
  `score_all`, same veto/tie-break ordering, same "no clean winner ⇒
  raw" outcome.
- **G6**: Leave the explicit `--type` path unchanged: it is an O(1) pool
  lookup with no sweep, and must not acquire a progress phase.

## Non-goals

- **N1**: Changing the scoring engine, `ScoringOpts`, or the veto/
  tie-break rule (G5 pins these).
- **N2**: Spec S1 of the scaling roadmap (seeding the Any/MessageSet
  auto-expand candidate list from `build_tree` to eliminate `App::new`'s
  full-document walk). Orthogonal and separately worthwhile; this spec
  neither needs nor blocks it.
- **N3**: Changing `DescriptorContext.graph` to `Arc<LoadedGraph>`
  (`rendering-flaws.md` A5). This spec removes the *one* thread that
  makes the current `&'static` unsound in practice, but the `pub`,
  `Copy`, mmap-derived field remains a latent hazard and should still be
  fixed on its own.
- **N4**: Making the sweep interruptible or cancellable. If the sweep
  proves slow enough to need that, the escape hatch is `--type` (G6) or
  a new opt-out flag, not a partial result.
- **N5**: Retroactively numbering the `spec NNNN` comments. This spec
  supersedes the behavior they describe; the comments are deleted along
  with the code they annotate, so no renumbering is needed.

## Open question — measure before implementing

This spec's entire case rests on the sweep being cheap relative to the
work it replaces. `resolve_root_winner_fqdn` generates no text and
builds no arena, so it should be well under `decode()`'s 1.65 s — but
that is an assumption, not a measurement.

**Before implementing, add a timing to
`protolens/src/tui/tests/profiling.rs` (throwaway/`#[ignore]`d; run with
`cargo test --release -p protolens --lib tui::tests::profiling --
--ignored --nocapture`) for `resolve_root_winner_fqdn` over the 1.1 MB
fixture and over
`googleapis.desc` (24.5 MB).** Proceed if the sweep is materially
cheaper than the ~10.6 s root re-splice it eliminates — which is a very
wide margin. If it somehow is not, the right change is instead to keep
the sweep asynchronous but have `RootTypeResolved` trigger a *re-decode*
rather than a root splice, since the splice path is what costs 10.6 s.

## Specification

### `protolens/src/decode.rs`

Delete the `defer_root_type` parameter from `decode()` and the
`root_type_deferred` field from `Decoded`. The `root_type_deferred`
branch at `:747-752` collapses back to an unconditional
`determine_root_type(blob, ctx, type_override)?`.

`resolve_root_winner_fqdn` stays exactly as it is — it remains
`determine_root_type`'s graph branch, which is its original role.

Give `determine_root_type` one new responsibility: when it takes the
graph branch, it must be able to hand back the full ranked candidate
list it already computed, not just the winner, so the caller can seed
the cache (G3). The smallest shape that does this without disturbing
the existing signature is a sibling function:

```rust
/// `resolve_root_winner_fqdn`'s cache-seeding counterpart: the same
/// single `score_all` sweep, returning both the winner (by the
/// identical veto/tie-break rule) and the full non-vetoed candidate
/// list. Exists so startup can populate `HeatCaches` for the root
/// range from the sweep it already had to run, rather than leaving
/// the override pane and the heat cue to re-score the same bytes.
pub(crate) fn resolve_root_winner_and_candidates(
    blob: &[u8],
    graph: &ArchivedCompiledGraph,
) -> (Option<String>, Vec<(String, i64)>);
```

`resolve_root_winner_fqdn` becomes a thin wrapper over it (`.0`), so
there is still exactly one copy of the sort comparator and the
tie-break rule.

### `protolens/src/tui/mod.rs`

Delete:

- the `root_type_pending: bool` field (`:801`) and its initializer
  (`:1149`),
- the `AppEvent::RootTypeResolved` variant and its handler
  (`:1844-1849`),
- `apply_resolved_root_type`,
- the whole `if app.root_type_pending { … thread::spawn … }` block
  (`:1673-1684`), together with the `let original_blob = …to_vec()`
  copy at `:1679` (which was also one of the three resident blob copies
  reported as P4).

### Startup sequence

The sweep must run before `decode()`, which means before `App::new`,
which means before the TUI is constructed — but *after* the terminal is
in its alternate screen, so G4's progress frame has somewhere to go.
Restructure `main.rs`'s TUI entry so the order is:

1. Enter raw mode / alternate screen (as `run()` does today at
   `mod.rs:1618-1623`).
2. Install the panic hook (today at `:1637-1641` — move it up so it
   covers everything below).
3. Draw a single progress frame: the splash (see
   `design/help-and-chrome.md`) plus a one-line status, e.g.
   `inferring root type… (24.5 MB)`.
4. Run `resolve_root_winner_and_candidates` on the **main thread**.
   No `stack_size` plumbing is needed: the main thread's stack is
   already larger than `thread::spawn`'s default — this is precisely why
   the pre-async synchronous path never needed `HEAT_WORKER_STACK_SIZE`.
5. Redraw the progress frame with `decoding…`.
6. `decode(blob, ctx, resolved_type.as_deref(), indent, …)` — one pass,
   correctly typed.
7. `App::new`, then seed the cache (below), then the event loop.

Steps 3-5 are skipped entirely when `--type` was given or no graph is
loaded (G6): there is no sweep to wait for.

### Cache seeding (G3)

Immediately after `App::new`, before `warm_up_heat_cues`:

```rust
// Spec 0168 G3: the root-type sweep already scored exactly the root
// node's interior payload range. Seed it at `Tier::User` so neither
// `heat_cue_for` nor the override pane re-runs `score_all` over the
// same bytes — this is the single most expensive range in the
// document, and it is also the one the cursor starts on.
```

Write both `by_range` (a `RangeHeatEntry` built from `derive_stats` plus
a `top_n` truncated to the same
`max(override_list_height, HEAT_CUE_PREVIEW)` cap `heat_cue_resolve`
uses) and `complete` (the full list, keyed by the root's range).

Note the interaction with `rendering-flaws.md` P5: seed `top_n` at the
capped width, *not* the full list, so this does not itself create the
oversized entry P5 describes.

### `run()`'s remaining structure

With the sweep moved out, `run()` keeps only the heat-worker spawn. The
two `?` early returns flagged as C4 (`:1631`, `:1694`) are now inside
the region the relocated panic hook covers, but `?` is not a panic —
close C4 in the same pass by wrapping the fallible middle of `run()` in
a closure whose result is captured before the cleanup block.

## Test plan

- Unit test: with a scoring graph and no `--type`, assert `decode()` is
  called exactly once during startup and that `Decoded::root_type` is
  the resolved FQDN — i.e. the document is never materialized under the
  raw fallback first. (Instrument via a counter in the profiling
  harness, as `TEST_INFERRED_CANDIDATES_CALLS` already does for
  `heat_worker`.)
- Unit test: with `--type` given, assert no sweep runs and no progress
  frame is drawn (G6).
- Unit test: with no scoring graph, assert startup is unchanged from
  today (raw root, no sweep, no progress frame).
- Unit test (G3): after startup, `HeatCaches::complete` holds the root
  node's range, and `by_range` holds an entry at the root's payload
  start whose `top_n.len()` is bounded by
  `max(override_list_height, HEAT_CUE_PREVIEW)`. Then open the override
  pane on the root and assert `score_all` is not re-invoked.
- Regression: `resolve_root_winner_fqdn`'s existing tests are unchanged
  (G5) — the wrapper must return exactly what it returned before for
  every existing case, including the no-winner and all-vetoed cases.
- Regression: the "no clean winner" path still yields the raw fallback
  root, and the override pane can still retype the root manually.
- Manual/perf (external fixtures, not in the automated suite): time
  startup to *first correct render* against `/tmp/pdb.desc` and
  `googleapis.desc`, before and after. The before number must include
  the `RootTypeResolved` re-splice, since that is when the display first
  becomes correct — comparing against the raw first paint would measure
  the wrong thing.
- Manual: quit (`q`) immediately at the splash on `googleapis.desc`,
  repeatedly, under a debugger. No SIGSEGV (C3 (a)); no stack overflow
  (C3 (b)).
