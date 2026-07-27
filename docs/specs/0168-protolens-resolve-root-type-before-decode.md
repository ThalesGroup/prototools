<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0168 — protolens: resolve the root type before decoding, not after

Status: implemented
App: protolens
Implemented in: 2026-07-27

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

  *Superseded during implementation*: spec 0180 landed first and did
  exactly this — the graph handed to a scoring thread is now an owning
  `Arc`, and the thread gets `SCORING_THREAD_STACK_SIZE`. So by the time
  this spec was implemented, deleting the root-type thread was no longer
  a *safety* fix; both halves of C3 were already closed. G2 is therefore
  "one fewer thread and one fewer full-document re-render", not "one
  fewer use-after-unmap".
- **N4**: Making the sweep interruptible or cancellable. If the sweep
  proves slow enough to need that, the escape hatch is `--type` (G6) or
  a new opt-out flag, not a partial result.

  *Resolved during implementation*: the flag exists and is `--raw`. It
  is not cancellation — it is a decision made before the sweep starts —
  but it covers the case the sweep is slow enough to matter, which on a
  large descriptor pool it is. See "The `--raw` escape hatch" below.
- **N5**: Retroactively numbering the `spec NNNN` comments. This spec
  supersedes the behavior they describe; the comments are deleted along
  with the code they annotate, so no renumbering is needed.

## Open question — measure before implementing — RESOLVED

This spec's entire case rested on the sweep being cheap relative to the
work it replaces. The sweep generates no text and builds no arena, so it
should be well under `decode()`'s 1.65 s — but that was an assumption,
not a measurement.

**Measured, and the gate passed by a wide margin.** The timing lives in
`protolens/src/tui/tests/profiling.rs` as a permanent `#[ignore]`d test
(run with `cargo test --release --bin protolens tui::tests::profiling --
--ignored --nocapture`): against the 1.1 MB `FileDescriptorSet` fixture
the sweep is **4% of the decode it precedes**, against a ~10.6 s
re-splice it was replacing. Implementation proceeded as specified.

That test is now kept for a second reason: it is the standing
measurement of the two costs `--raw` lets a user trade off.

## Specification

### `protolens/src/decode.rs`

Delete the `defer_root_type` parameter from `decode()` and the
`root_type_deferred` field from `Decoded`. The `root_type_deferred`
branch at `:747-752` collapses back to an unconditional
`determine_root_type(blob, ctx, type_override)?`.

Give `determine_root_type` one new responsibility: when it takes the
graph branch, it must hand back the full ranked candidate list it
already computed, not just the winner, so the caller can seed the cache
(G3):

```rust
/// The same single `score_all` sweep, returning both the winner (by
/// the veto/tie-break rule, unchanged) and the full non-vetoed
/// candidate list. Exists so startup can populate `HeatCaches` for the
/// root range from the sweep it already had to run, rather than
/// leaving the override pane and the heat cue to re-score the same
/// bytes.
pub(crate) fn resolve_root_winner_and_candidates(
    blob: &[u8],
    graph: &ArchivedCompiledGraph,
) -> (Option<String>, RankedCandidates);
```

`RankedCandidates` is a `pub type` alias for `Vec<(String, i64)>` — the
shape `override_pane::inferred_candidates` and `heat_worker::HeatCaches`
both already traffic in, named so the signatures threading it through
say what they carry (and so `determine_root_type`'s return type is not a
`clippy::type_complexity` violation).

**Divergence: `resolve_root_winner_fqdn` is deleted, not kept as a
wrapper.** The spec planned to keep it returning `.0`. But with the
candidate list flowing all the way out of `determine_root_type`, the
winner-only form had no remaining caller outside its own tests, and a
`pub fn` in a bin crate that only tests reach is dead code the compiler
correctly complains about. Deleting it is what keeps exactly one copy of
the sort comparator and the tie-break rule, which is what the wrapper
was for.

**The three startup type modes become one enum.** `decode()` took
`type_override: Option<&str>` *plus* `defer_root_type: bool`, a pair
that makes "a named type, but also deferred" expressible and
meaningless. Since `--raw` (below) needs precisely the decode-side
meaning the `defer` flag had, the flag could not simply be deleted; the
pair is replaced by

```rust
pub enum RootType<'a> { Infer, Named(&'a str), Raw }
```

so the three modes are exhaustive and the impossible combination cannot
be written.

**`decode()` splits into `determine_root_type` + `render_resolved`.**
Both halves are multi-second on a large blob and they are wildly
unequal, so the startup messages (below) must name them separately;
`main` therefore needs to call them one at a time. `decode()` remains,
calling both, for every other caller — including the batch subcommands,
which print no progress and so have no reason to hold the halves apart.

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
which means before the TUI is constructed. All of it therefore happens
in `main.rs`, before `tui::run` is ever called.

**Divergence: G4's progress indication is stderr lines, not an
in-terminal frame.** The spec prescribed entering the alternate screen
first and drawing a splash plus a one-line status, redrawn per phase.
That was rejected in favor of plain `eprintln!` lines, because startup
already prints such lines for the phases that precede the sweep
(descriptor-set load, terminal-capability detection) and a frame would
have to be torn back down before `tui::run` re-enters the screen itself.
Ordinary scrollback also survives the run, so a slow startup leaves a
record of *which* phase was slow.

The order is:

1. Announce and run `determine_root_type` — the sweep, on the **main
   thread**. No `stack_size` plumbing is needed: the main thread's stack
   is already larger than `thread::spawn`'s default; this is precisely
   why the pre-async synchronous path never needed
   `HEAT_WORKER_STACK_SIZE`.
2. Announce and run `render_resolved` — one pass, correctly typed.
3. Announce and run `App::new`, then seed the cache (below), then
   `tui::run`.

Each phase is announced *before* it runs and named for what it is
about to do, with the blob size where the cost scales with it:

```
protolens: loading descriptor set 'pdb.desc' (1 MB)...
protolens: inferring root type (24 MB)...
protolens: rendering root node as google.protobuf.FileDescriptorSet (24 MB)...
protolens: indexing 193072 lines...
protolens: detecting terminal capabilities...
```

Three separate lines for sweep, render, and index, rather than one
spanning them, because all three are multi-second and wildly unequal —
on a 24 MB blob against a small graph the whole startup is ~27 s and the
sweep is a few seconds of it. A single message spanning two phases makes
whichever one is actually running look like a hang in the other. This is
the requirement that forces the `decode()` split above.

The `rendering root node as …` line names the resolved type, so the
answer the sweep produced is visible before the render that depends on
it starts, and a wrong inference is diagnosable without waiting for the
render to finish. With no type it reads `<raw / no type>`.

The scoring graph is deliberately *not* named or sized in the load line:
it runs about a fifth of the descriptor set's size (4.6 MB against
24.5 MB for googleapis; 209 KB against 1.0 MB for a protoc descriptor
pool), so the descriptor size alone already says what the wait is
proportional to, and whether a graph was found is legible from the two
lines that follow.

The sweep line is skipped entirely when `--type` or `--raw` was given or
no graph is loaded (G6): there is no sweep to wait for. Batch
subcommands print no progress at all.

### The `--raw` escape hatch

`--raw` opens the blob with no type at all, as raw wire bytes,
conflicting with `--type`. It is `RootType::Raw`, which is exactly what
the deleted `defer_root_type` flag meant on the decode side — the
difference is that the render now *stays* raw instead of being replaced
under the reader seconds later.

It exists because inference is the one startup phase whose cost scales
with the size of the schema *database* rather than the blob, so on a
large descriptor set it is the fast way in; it is also the way to look
at a blob whose inferred type is wrong. A type can still be chosen
interactively afterwards, via the override pane, which is the same path
a user takes to correct any other node.

### Cache seeding (G3)

**Divergence: the seeding lives at the end of `App::new` itself**, as
`App::seed_root_heat`, rather than in `main` after it returns. The
candidate list rides in on `Decoded::root_candidates` and is
`mem::take`n out of it, so the caller has nothing to remember to do and
the two batch subcommands — which construct a `Decoded` and never build
an `App` — are unaffected. Nothing else about the seeding changed.

The key is the sweep's own input, verified rather than assumed:
`heat_scored_range(first_node)` resolves to
`wrapper_offset..blob.len()`, which is exactly the byte range
`resolve_root_winner_and_candidates` scored. That identity is what makes
the seeding sound; if it ever stops holding, the seeded entry would
answer a question nobody asked.

A no-op when the sweep did not run (`--type`, `--raw`, or no graph),
which is why an empty list is not special-cased anywhere else.

At `App::new` time `override_list_height` is still 0, so the cap below
evaluates to `HEAT_CUE_PREVIEW`.

The intent, as specified:

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

With the sweep moved out, `run()` keeps only the heat-worker spawn.

The spec also asked to close flaw C4 (the `?` early returns that skip
terminal cleanup) in the same pass. **Already done** — worklist item W3
landed it first; `mod.rs:1839` installs the panic hook before the first
fallible call and `:1875` wraps everything fallible up to `run_loop` so
its result is captured before the cleanup block. No work here.

## Test plan

Implemented, in `decode.rs`'s own test module:

- `raw_never_sweeps_even_when_a_graph_is_loaded` — `RootType::Raw`
  returns no winner *and* an empty candidate list with a real graph
  attached. The empty list is the assertion that matters: a sweep that
  ran would have produced entries, so this pins that `--raw` skips the
  work rather than discarding its result.
- `a_named_type_is_looked_up_not_swept_and_a_bad_name_errors` (G6) — an
  unknown `--type` is a `DecodeError::Determination`, not a quiet
  fallback to inference, which would make a typo look like a scoring
  bug.
- `named_types_the_root_and_raw_leaves_it_untyped` — the two explicit
  modes over one blob and one pool: `Named` yields `root_type ==
  "test.Inner"`, `Raw` yields `"<raw / no type>"`, neither carries
  candidates. This is `--raw`'s real guarantee — the render *stays*
  raw — which is what distinguishes it from the deleted
  `defer_root_type`.
- `determine_root_type_returns_none_without_override_or_graph`
  (pre-existing, extended) — no graph means no sweep ran.

Implemented, in `tui/tests/heat_cue.rs`:

- `g3_the_startup_sweep_seeds_the_root_range` — after `App::new`,
  `by_range` holds an entry at the root's payload start with the
  sweep's stats, `complete` holds the full list keyed by the root's
  range, and the root's first `heat_cue_for` is a `Cue`. Run against a
  graph-less `App`, where a miss could only short-circuit to `None`, so
  a `Cue` proves the seed was used and no re-score happened.
- `g3_seeded_top_n_is_capped_but_complete_is_not` — `top_n.len() ==
  HEAT_CUE_PREVIEW` (the cap at `App::new` time, when
  `override_list_height` is still 0) while `complete` keeps every
  candidate. Pins both halves of the P5 interaction.
- `g3_no_sweep_seeds_nothing` — an empty candidate list writes nothing.
  Writing an empty entry would read as a legitimate "every candidate
  vetoed" answer and permanently suppress the root's cue.

Not implemented, and why:

- "Assert `decode()` is called exactly once during startup." There is
  no longer anything that could call it twice: `AppEvent::
  RootTypeResolved` and `apply_resolved_root_type` are deleted, so the
  second pass is gone by construction rather than by policy. A
  call-counter would be testing the absence of deleted code.
- "Assert no progress frame is drawn." Moot — G4 is stderr lines
  emitted from `main`, not a frame (see Startup sequence). The
  no-sweep half of the same claim is covered by the candidate-list
  assertions above.
- `resolve_root_winner_fqdn`'s regression tests — the function is
  deleted; `resolve_root_winner_and_candidates`, which holds the only
  copy of the comparator and tie-break rule, is exercised by the
  profiling gate below.

Manual, run outside the automated suite:

- Perf: startup to *first correct render*, before and after. The before
  number must include the `RootTypeResolved` re-splice, since that is
  when the display first becomes correct. The standing in-suite proxy is
  the `#[ignore]`d profiling gate, which reports the sweep at 4% of the
  decode it precedes.
- Quit (`q`) immediately at the splash on `googleapis.desc`. No
  SIGSEGV, no stack overflow — though as noted under N3, spec 0180
  already closed both hazards before this spec landed.
