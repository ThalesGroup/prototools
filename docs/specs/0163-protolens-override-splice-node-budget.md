<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0163 — protolens: cap a single override splice's decode/render size

Status: implemented
App: protolens
Implemented in: 2026-07-24

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
  every *live-preview* candidate decode it performs — the speculative
  reinterpretation under a caller-chosen candidate type that may not
  match the actual encoding. A *confirmed* override (the type actually
  applied, not merely being previewed) is exempt and always renders
  completely — see the revised N2 below.
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
  node's own byte length or the document's current size). ~~A fixed
  constant is used initially; tuning or adaptivity is left for
  follow-up if the fixed value proves wrong in practice.~~ **Extended
  during implementation** (2026-07-24 follow-up feedback): the budget
  is still a single flat number, not adaptive, but it is no longer a
  hardcoded constant — `App::override_splice_node_budget` is a plain
  field (defaulting to `OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT`, now
  `200`, down from the constant's earlier `1,000`), settable at
  startup via `main.rs`'s `--override-preview-node-budget` CLI flag.
  Still no *adaptive* scaling — this only lets a user override the
  one flat default for their own session, N1's actual substance is
  unchanged.
- **N2**: ~~A different (e.g. higher) budget for a confirmed/committed
  override versus a live preview. Every `splice_override` call, preview
  or real, shares the same budget for simplicity (see G2).~~ **Reversed
  during implementation** (2026-07-24 feedback): a confirmed override
  is the content actually shown as the document's real rendering, not a
  speculative guess — truncating it would silently hide data. Only a
  live preview (`splice_override`'s new `is_preview: true` parameter,
  `preview_override_highlight`'s sole call site) is budget-capped; every
  other call site (routed through `resettle_node`, i.e. an
  already-confirmed/active override being (re)applied) passes
  `is_preview: false` and gets `node_budget: None` (unbounded), exactly
  like every non-`protolens` caller (G3).
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

**Found during implementation**: `render_message`'s loop counts a
packed-repeated scalar field (`ScalarValue::Packed`) as a single
iteration, regardless of how many elements it decodes into.
`render_packed` (`prototext-core/src/serialize/render_text/
packed.rs`) and `IndexingTextSink::scalar_field`'s packed-element
`NodeSpan`-building loop (`sink.rs`) each independently iterate over
every decoded packed element — a candidate that mis-parses as one
enormous packed field bypasses the budget entirely through these two
uninstrumented loops (confirmed via the Test plan's manual/perf
validation step below: the fix as first implemented did not bound
`/tmp/db3.desc`'s pathological candidate). Fixed by mirroring the same
`NODE_BUDGET`/`NODE_COUNT` check inside `render_packed`'s per-element
loop (aborting via the same `NodeBudgetExceeded` annotation), and by
having `IndexingTextSink::scalar_field` snapshot `NODE_COUNT` before
and after the delegated call to detect and match that same truncation
point when building spans, instead of re-deriving the full element
list independently.

### protolens/src/tui/override_apply.rs

New associated constant near `splice_override`, and a matching
`App` field seeded from it (**extended from this spec's original N1**
during implementation, see above — first landed as a plain `const`,
then turned into a field so it can be overridden per-session):

```rust
/// Spec 0163: default for `App::override_splice_node_budget` — the
/// maximum number of fields/spans a single *live-preview*
/// `splice_override` candidate decode may produce before the rest is
/// shown as `MalformedKind::NodeBudgetExceeded` instead of continuing
/// to recursively decode/render — guards against a structurally
/// mismatched candidate type causing the recursive-descent decoder
/// to mis-parse arbitrary bytes into a pathologically large synthetic
/// tree (observed: 1,083,626 spans from a single splice on a ~1.1MB
/// field — larger than the entire 635,052-node original document).
/// Only applies to a live preview (see `is_preview` below) — a
/// preview only needs to show enough of a wrong-type candidate's
/// shape for the user to judge it's the wrong one and move on, so
/// this is kept low (200) rather than matched to the ~50k+ scale of
/// the pathological blowups it guards against. Overridable at
/// startup via `--override-preview-node-budget`.
pub(crate) const OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT: usize = 200;
```

`App` gains an `override_splice_node_budget: usize` field, seeded to
`OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT` in `App::new`.

`splice_override` gains an `is_preview: bool` parameter: `true` from
`preview_override_highlight`'s sole live-preview call site, `false`
from every other call site (routed through `resettle_node`, i.e. an
already-confirmed/active override being (re)applied — **reversed from
this spec's original N2** during implementation, see above). Its
`DecodeRenderOpts` construction (the cache-miss branch) gains:

```rust
node_budget: is_preview.then_some(self.override_splice_node_budget),
```

`RenderCache`'s key (`protolens/src/render_cache.rs`) gains `is_
preview: bool` as a third tuple element: a budget-truncated preview
render and a full confirmed render of the same `(range, target)` must
never be conflated, or confirming an override could silently reuse a
truncated preview render cached by an earlier `Down`/`Up` cycle over
the same candidate.

### protolens/src/main.rs

New `Cli` field, defaulting to `App::OVERRIDE_SPLICE_NODE_BUDGET_
DEFAULT`, assigned onto `app.override_splice_node_budget` right after
`App::new`:

```rust
#[arg(
    long = "override-preview-node-budget",
    default_value_t = tui::App::OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT,
)]
override_preview_node_budget: usize,
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
  `OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT` spans; confirm a *preview*
  (`is_preview: true`) `splice_override` completes (doesn't hang/panic)
  and the resulting tree/lines are bounded in size, with a visible
  budget-exceeded marker in the rendered output.
- Companion `protolens` test (added when N2 was reversed): the same
  pathological candidate spliced as a *confirmed* override
  (`is_preview: false`) instead — must render completely, with
  `tree.len()` reaching the full mis-parsed field count and no
  `NODE_BUDGET_EXCEEDED` marker anywhere in the output.
- Companion `protolens` test (added when N1 was extended): setting
  `app.override_splice_node_budget` to a custom value below the
  default actually changes where a live-preview splice truncates,
  confirming the field (not just its default) is load-bearing.
- Regression: existing `tui/tests/override_apply.rs`/`override_
  select.rs` suites must pass unchanged — no existing fixture is
  anywhere near 200 spans for a single field, so no existing test's
  candidate should ever trip the default budget.
- Manual/perf validation (external fixture, not part of the automated
  suite): re-run `tests/profiling.rs`'s `Down`-press loop against
  `/tmp/db3.desc` and confirm the previously-observed pathological
  candidate (1,083,626 spans) now renders in bounded time instead of
  contributing to the multi-second/multi-ten-second stalls. This step
  is what surfaced the packed-field gap noted above under
  Specification — the first implementation attempt (budget checked
  only in `render_message`'s per-field loop) left `Down` presses at
  8-16s+ with `tree.len()` still reaching 1,718,678. After also
  covering `render_packed`'s and `IndexingTextSink::scalar_field`'s
  per-packed-element loops, `Down` dropped to ~0.8-2.5s with
  `tree.len()` bounded to the base document size plus the (then
  50,000) budget. After N2 was reversed and the budget lowered to
  1,000, re-running again showed `Down` at ~2-70ms (one ~1.5s outlier,
  attributable to the separate, out-of-scope N3 cost) with `tree.len()`
  bounded to 635,052 + 1,000 = 636,050 — confirmed exact. After N1 was
  extended (default further lowered to 200, and made CLI-overridable),
  the same bound now tracks 635,052 + 200 = 635,252 at the default.
