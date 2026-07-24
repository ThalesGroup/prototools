<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0163 — protolens: cap a single override splice's decode/render size

Status: draft
App: protolens

## Background

The override pane's live preview (`preview_override_highlight`,
`override_select.rs`) reinterprets a node's raw tag+payload bytes under
whichever candidate type is currently highlighted, by calling
`self.splice_override(idx, tentative)` (`override_apply.rs`). On a
cache miss, this decodes the bytes fresh against the candidate's
synthetic wrapper descriptor via `prototext_core`'s recursive-descent
decoder (`decode_and_render_indexed`, `prototext-core/src/serialize/
render_text/mod.rs`, `render_message`'s per-field loop, `mod.rs:364+`).

Nothing bounds how large this speculative reinterpretation can grow.
Protobuf's wire format is permissive enough that reinterpreting a large
byte blob under a structurally mismatched candidate type can still
"successfully" parse — the decoder doesn't reject bytes that merely
happen to form syntactically valid (if semantically meaningless)
tag/length/value sequences under the wrong schema, so a wrong-type
candidate over a large enough payload can recursively mis-parse
essentially every byte into spurious nested submessage spans.

### Confirmed by direct instrumentation

Using an interactive-path-faithful profiling harness
(`protolens/src/tui/tests/profiling.rs`, throwaway/`#[ignore]`d)
against a large real-world descriptor set (`/tmp/db3.desc`, decoding to
635,052 tree nodes for a ~1.1MB blob): one of the resolved candidates
for a field whose own raw payload spans nearly the entire document
mis-parses that payload into **1,083,626 spans from a single splice**
— more spans than the entire correctly decoded 635,052-node original
document. Cycling to that candidate (a single `Down` press in the
override pane) pays the full cost of decoding, rendering, and
`Vec::push`-ing all ~1.08M of those spans into `self.tree`/`self.lines`
in one call, measurably contributing multiple seconds to that single
keystroke — independent of, and compounding with, the separate
across-preview accumulation problem addressed by spec 0161.

Neither `decode_and_render`/`decode_and_render_indexed`
(`prototext-core`) nor `splice_override` (`protolens`) have any notion
of a size ceiling today — a candidate is always decoded and rendered
to completion, however large the result turns out to be, before
`splice_override` can even measure its size.

## Goals

- **G1**: `decode_and_render`/`decode_and_render_indexed` gain an
  opt-in budget (`DecodeRenderOpts::node_budget: Option<usize>`) on
  the number of fields/spans a single decode may emit before aborting
  the recursive descent early, rendering the undecoded remainder as a
  distinct, clearly-labeled annotation (reusing the existing
  `Sink::malformed`/`MalformedKind` mechanism already used for
  genuinely corrupt encodings) rather than continuing to recursively
  decode/render an unbounded amount of speculative content.
- **G2**: `protolens`'s `splice_override` passes a concrete budget for
  every candidate decode it performs (both live-preview and confirmed
  real-override splices) — the only call site that speculatively
  reinterprets bytes under a caller-chosen candidate type that may not
  match the actual encoding.
- **G3**: `DecodeRenderOpts::node_budget` defaults to `None`
  (unlimited, today's behavior) — every other caller (the `prototext`
  CLI, `protolens`'s own initial whole-document decode in
  `decode::decode`, `command_line.rs`'s export path) is unaffected,
  since those decode the document's own real, already-established
  type, not a speculative guess.
- **G4**: A budget-truncated decode remains well-formed enough for
  `protolens` to build a tree from and splice in normally — no panics,
  no dangling/malformed spans — degrading to the same "malformed data"
  visual treatment already used for a genuinely corrupt encoding, not
  a crash or a blank pane.
- **G5**: Bound not just this splice's own decode/render time but also
  its lasting footprint in `self.tree`/`self.lines` — a budget-capped
  candidate can never contribute more nodes/lines than the configured
  ceiling, complementing (not duplicating) spec 0161's separate fix
  for garbage *accumulating across* repeated previews.

## Non-goals

- **N1**: Choosing a smarter/adaptive budget (e.g. scaled to the
  node's own byte length or the document's current size). A fixed
  constant is used initially; tuning or adaptivity is left for
  follow-up if the fixed value proves wrong in practice.
- **N2**: A different (e.g. higher) budget for a confirmed/committed
  override versus a live preview. Every `splice_override` call, preview
  or real, shares the same budget for simplicity (see G2).
- **N3**: The separate, already-present cost of `finalize_override_
  batch` (`line_to_node`/`footer_line_to_node` full rebuild,
  `rebuild_visible_rows`'s full `Vec` rebuild — `override_apply.rs:
  901-916`, `navigation.rs:37-49`) running unconditionally, at
  whole-document cost, on *every* standalone `splice_override` call
  regardless of splice size. This was confirmed during this
  investigation to be untouched by spec 0160 for standalone
  (non-batch) calls — 0160 only reduced repeat calls *within one
  outer `render_overrides` batch*, not the per-call cost of a
  standalone call, which was and remains "once per splice". This is a
  distinct, likely substantial contributor to `t`'s own reported
  multi-second latency even on a legitimate (non-blowup) first
  preview on a large document, but a genuine fix appears to need a
  materially larger, non-flat document-representation redesign rather
  than a targeted change, and is intentionally left out of this spec.
- **N4**: Any change to how candidates are scored/sorted/ordered in
  the override pane's list (`heat_worker`/spec 0152 territory) —
  unaffected.

## Specification

### prototext-core/src/serialize/render_text/sink.rs

New `MalformedKind` variant, alongside the existing ones:

```rust
/// Spec 0163: the configured `DecodeRenderOpts::node_budget` was
/// reached mid-decode — the remaining bytes are rendered as this
/// annotation instead of continuing to recursively decode them.
NodeBudgetExceeded,
```

A rendering arm mirroring the existing `TruncatedBytes` arm, emitting
a one-line annotation naming how many trailing bytes were left
undecoded (e.g. `# malformed: node budget exceeded, N byte(s) not
rendered (likely a type mismatch)`).

### prototext-core/src/serialize/render_text/mod.rs

`DecodeRenderOpts` gains:

```rust
/// Spec 0163: maximum number of fields/spans this decode may emit
/// before aborting early and marking the remainder
/// `MalformedKind::NodeBudgetExceeded`, instead of continuing to
/// recursively decode/render an unbounded amount of content. `None`
/// (the default) preserves today's unlimited behavior.
pub node_budget: Option<usize>,
```

Defaulted to `None` in `impl Default for DecodeRenderOpts`.

New thread-locals alongside the existing `LEVEL` counter:

```rust
pub(super) static NODE_BUDGET: Cell<Option<usize>> = const { Cell::new(None) };
pub(super) static NODE_COUNT:  Cell<usize>         = const { Cell::new(0) };
```

`decode_and_render`/`decode_and_render_indexed` each set/reset both,
alongside their other `*.with(|c| c.set(...))` initialization lines:

```rust
NODE_BUDGET.with(|c| c.set(opts.node_budget));
NODE_COUNT.with(|c| c.set(0));
```

`render_message`'s per-field loop gains a check immediately after the
existing `if pos == buflen { return (pos, None); }` guard, before
parsing the next tag:

```rust
if let Some(budget) = NODE_BUDGET.with(Cell::get) {
    let count = NODE_COUNT.with(Cell::get) + 1;
    NODE_COUNT.with(|c| c.set(count));
    if count > budget {
        sink.malformed(
            0,
            TagFacts::default(),
            MalformedKind::NodeBudgetExceeded,
            &buf[pos..],
        );
        return (buflen, None);
    }
}
```

This mirrors the existing `InvalidTagType` early-return exactly —
the rest of the buffer is treated as unparsed, the same handling
already used for any other malformed-data case, so no new control-flow
shape is introduced. The exact set of call sites where the counter
needs incrementing (this top-level loop covers every scalar and
every entry into a nested message/group, since `render_len_field`/
`render_group_field` ultimately recurse back into `render_message`)
is confirmed during implementation.

### protolens/src/tui/override_apply.rs

New constant near `splice_override`:

```rust
/// Spec 0163: maximum number of fields/spans a single `splice_
/// override` candidate decode may produce before the rest is shown
/// as `MalformedKind::NodeBudgetExceeded` instead of continuing to
/// recursively decode/render — guards against a structurally
/// mismatched candidate type causing the recursive-descent decoder
/// to mis-parse arbitrary bytes into a pathologically large synthetic
/// tree (observed: 1,083,626 spans from a single splice on a ~1.1MB
/// field — larger than the entire 635,052-node original document).
const OVERRIDE_SPLICE_NODE_BUDGET: usize = 50_000;
```

`splice_override`'s `DecodeRenderOpts` construction (the cache-miss
branch) gains:

```rust
node_budget: Some(OVERRIDE_SPLICE_NODE_BUDGET),
```

## Test plan

- New unit test in `prototext-core`'s render_text test suite: a
  deeply/repeatedly nested byte sequence decoded with a small
  `node_budget` (e.g. `10`) produces a bounded number of spans/lines,
  ending in a `NodeBudgetExceeded` annotation — no panic, no unbounded
  output.
- New unit test confirming `node_budget: None` (the default) is fully
  unaffected — byte-for-byte identical output to today's behavior for
  both `decode_and_render` and `decode_and_render_indexed`, on an
  existing large fixture.
- New `protolens` test in `tui/tests/override_apply.rs`: a payload/
  candidate-type combination engineered to mis-parse into more than
  `OVERRIDE_SPLICE_NODE_BUDGET` spans; confirm `splice_override`
  completes (doesn't hang/panic) and the resulting tree/lines are
  bounded in size, with a visible budget-exceeded marker in the
  rendered output.
- Regression: existing `tui/tests/override_apply.rs`/`override_
  select.rs` suites must pass unchanged — no existing fixture is
  anywhere near 50,000 spans for a single field, so no existing test's
  candidate should ever trip the new budget.
- Manual/perf validation (external fixture, not part of the automated
  suite): re-run `tests/profiling.rs`'s `Down`-press loop against
  `/tmp/db3.desc` and confirm the previously-observed pathological
  candidate (1,083,626 spans) now renders in bounded time instead of
  contributing to the multi-second/multi-ten-second stalls.
