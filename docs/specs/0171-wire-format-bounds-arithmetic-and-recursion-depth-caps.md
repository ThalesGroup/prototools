<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0171 — prototext-core, prototext-graph: wire-format bounds arithmetic and recursion depth caps

Status: implemented
Implemented in: 2026-07-25
App: prototext-core, prototext-graph
Refs: docs/prototext/decode-flaws.md (C1, C2, C3, C5),
      docs/scoring-flaws.md (C1, C3),
      docs/protolens/rendering-worklist.md (W26, W27, W28),
      docs/specs/0110-sink-based-render-unification.md
      (the `Sink` contract and the write-in-place invariant S3 preserves)

## Background

Two crates independently decode the protobuf wire format:
`prototext-core`'s `serialize::render_text` (render-while-decode) and
`prototext-graph`'s `score::walk` (score-while-decode). Neither shares
code with the other — `prototext-graph` deliberately reimplements varint
and tag parsing so it can run allocation-free, where `prototext-core`'s
`VarintResult` carries a `Vec<u8>` of garbage bytes.

They independently wrote the same two bugs.

### 1. The length check overflows

Twenty-five sites across both crates spell the LEN/fixed-width bounds
check as

```rust
if pos + length > buflen { /* reject */ }
let data = &buf[pos..pos + length];
```

`length` comes straight off the wire as a u64 and is cast to `usize`.
A length prefix near `u64::MAX` makes `pos + length` wrap in a release
build (`overflow-checks` is off), the guard passes, and the very next
line slices `buf[pos..pos + length]` with `start > end` — a panic, from
the crate whose entire premise is that no byte sequence is unrenderable.
In a debug build it is an arithmetic-overflow panic one line earlier.

Confirmed sites: `render_text/mod.rs:540`,
`helpers/any_field.rs:70`/`:150`, `helpers/message_set_field.rs:105`/
`:155`, `score/walk.rs:453`/`:669`/`:734`/`:1031`.

The remaining seventeen are the fixed-width forms `pos + 8 > buflen` and
`pos + 4 > buflen`. Those cannot actually wrap — `buflen <= isize::MAX`
for any real slice, so `pos + 8` has room — but they are the same idiom
written by the same hand, and leaving half the sites using a checked
helper and half using raw addition means the next reader has to redo
this analysis to know which is which. They are converted too.

### 2. Blind group skipping recurses without a cap

Both crates have a "consume bytes until the matching `END_GROUP`"
routine used when there is no schema to steer with:
`any_field.rs:116` (`skip_group`) and `walk.rs:421`
(`parse_group_blind`). Both recurse on `WT_START_GROUP` with no depth
parameter. A `START_GROUP` tag costs one byte, so a 1 MB input can
demand a million stack frames. `score_message_multi` *is* capped
(`MAX_SCORE_DEPTH = 1000`, `walk.rs:770`) but delegates to the uncapped
`parse_group_blind` whenever every active candidate is `Unknown`.

`prototext-core`'s main path has no cap either — not on groups and not
on LEN nesting. `render_message` → `render_len_field` → `render_message`
recurses once per nesting level, and a LEN nesting level costs two bytes
(tag + length prefix), so a 1 MB blob can nest 500 000 levels deep.
Grepping `render_text/` for a depth limit finds only `LEVEL`, which is
the *indentation* counter and is never compared to anything. The one
existing brake, spec 0163's `node_budget`, is `None` in protolens's
production options (`protolens/src/decode.rs:769-787`) and is in any
case a budget on *output size*, not on stack.

### 3. A cap must not cost the rest of the buffer

The obvious cap — "refuse to recurse, abandon the buffer" — is wrong
here, because hitting the cap says nothing about whether the input is
legitimate. A conforming encoder can nest 1 001 levels; that payload is
perfectly decodable, it merely exceeds what this decoder is willing to
spend stack on. Abandoning everything after it would make a local
resource limit look like global corruption, and would lose every
sibling field that follows.

So the cap must be *local*: render the one over-deep node opaquely, then
carry on with the parent's loop as if nothing had happened. The two
recursion sites differ in how hard that is:

- A **LEN** nested message carries its own length prefix, so its extent
  is already known. Rendering it as opaque bytes is ordinary grammar and
  needs no new machinery — it is the same `ScalarValue::Bytes` outcome
  the spec-0097 cascade reaches at step 3 for any payload it cannot
  claim is a message.
- A **GROUP** carries no length prefix, so its extent is unknowable
  without walking to the matching `END_GROUP`. That walk is the only new
  mechanism this spec needs.

### 4. The write-in-place invariant

`TextSink` renders straight into one output buffer: every method
appends at its final position, and the sole retro-fit is `end_nested`'s
`splice` at a remembered `header_nl_pos` for group close facts, which
shifts `CBL_START` by the inserted length (`sink.rs:715-719`). There is
no shadow buffer and no render-then-copy anywhere in the pass.

A cap implemented as "start rendering, discover the depth, retract" would
break that. S4 is therefore arranged so the depth decision is made
*before* `begin_nested` at both sites — nothing is ever written that has
to be taken back.

## Goals

- **G1**: One checked bounds-arithmetic helper, used by every wire-format
  length check in both crates, so a hostile length prefix can never
  produce a wrapped `pos + length` or an inverted slice range.
- **G2**: No wire-format walk in either crate can exhaust the stack.
  `render_message`'s recursion is genuine and gets a hard depth cap; the
  blind group walkers (`skip_group`, `parse_group_blind`) do not need to
  recurse at all and are rewritten iteratively.
- **G3**: Hitting the cap costs exactly the over-deep node and nothing
  else: its siblings, and every level above it, still render.
- **G4**: No new rendered grammar. The cap reports through productions
  the encoder already round-trips, so a capped render still re-encodes
  byte-for-byte.

## Non-goals

- **N1**: Unifying the two crates' varint/tag parsers.
  `prototext-graph`'s allocation-free `VarintResult`/`TagResult` exist
  for a reason (`walk.rs:29-36`); merging them is a separate,
  performance-sensitive change. Only the *arithmetic* is shared.
- **N2**: A shared group scanner. It follows from N1: the two copies
  differ only in which parser result type they destructure, and unifying
  them requires unifying the parsers first. Each crate rewrites its own
  copy iteratively.
- **N3**: Anything to do with `node_budget` — neither its interaction
  with `ProbeSink` (a real bug: the probe's fields are charged to the
  outer render's budget, so a tripped budget silently demotes
  well-formed nested messages to bytes) nor moving it out of
  `prototext-core` altogether. Both are their own specs. The depth cap
  is a property of the decoder; the node budget is a caller-set option,
  and this spec deliberately does not touch that interface.
- **N4**: Changing what `node_budget` renders as. That is spec 0170.
- **N5**: `cargo-fuzz` targets. Both flaws reports recommend them and
  they remain recommended; this spec ships the deterministic tests that
  pin the specific defects, and the fuzzing harness is follow-on work
  with its own build-system footprint.
- **N6**: Making the depth cap configurable. It is a compile-time
  constant on purpose — see S3's rationale for why that distinction
  matters.

## Specification

### S1. `prototext-core::helpers::bounds`

New module `prototext-core/src/helpers/bounds.rs`, re-exported by
`helpers/mod.rs` (`pub use bounds::*;`) alongside `codecs`, `varint`, and
`wire`. `prototext-graph` already depends on `prototext-core`, so it
reaches these through `prototext_core::helpers::{payload_end,
MAX_WIRE_DEPTH}`.

```rust
/// End offset of a wire-format payload of `len` bytes starting at `pos`,
/// or `None` if it does not fit within `buflen`.
///
/// The naive form of this check — `if pos + len > buflen` — is wrong for
/// a length read off the wire: `len` is an attacker-chosen u64, the sum
/// wraps in a release build, the guard passes, and the caller then
/// slices `buf[pos..pos + len]` with `start > end`. Every wire-format
/// length check in this workspace goes through here so that the wrapping
/// form has no place left to reappear.
///
/// `pos > buflen` also yields `None` rather than panicking, so callers
/// need no separate invariant check; in practice every caller maintains
/// `pos <= buflen` because `parse_varint` and `parse_wiretag` both clamp
/// `next_pos` to `buflen`.
#[inline]
pub fn payload_end(pos: usize, len: u64, buflen: usize) -> Option<usize> {
    if pos > buflen {
        return None;
    }
    // `buflen - pos` cannot underflow, and comparing against the
    // remaining span rather than forming `pos + len` is what makes the
    // check total over u64.
    if len > (buflen - pos) as u64 {
        return None;
    }
    Some(pos + len as usize)
}

/// Hard cap on wire-format walk recursion depth, shared by every decoder
/// and scorer in this workspace.
///
/// Nesting depth on the wire is bounded only by the input's length: a
/// LEN nesting level costs two bytes (tag + length prefix) and a
/// `START_GROUP` level costs one, so a 1 MB blob can demand hundreds of
/// thousands of stack frames. 1000 is far beyond any legitimate schema's
/// nesting depth — protobuf's own reference implementations default to
/// 100 — while staying comfortably inside a default thread stack.
///
/// It is deliberately a compile-time `const` rather than an option. A
/// caller-tunable depth would make a rendering a function of
/// `(bytes, schema, depth)`, breaking the property the whole override
/// model rests on: that the same bytes always render the same way.
pub const MAX_WIRE_DEPTH: usize = 1000;
```

A companion `bytes_missing(pos, len, buflen) -> u64` yields the
`missing` count for `TRUNCATED_BYTES` in the same module, so the
shortfall is not recomputed by hand at the one site that needs it
(`render_text/mod.rs:617`). It is written with saturating subtraction
throughout: the arithmetic is only meaningful after `payload_end` has
already returned `None`, and saturating rather than asserting that
precondition keeps the helper total.

### S2. Convert every bounds check

All twenty-five sites listed in Background §1 become

```rust
let Some(end) = payload_end(pos, len, buflen) else { /* existing rejection */ };
let payload = &buf[pos..end];
pos = end;
```

with the fixed-width forms passing a literal `8` / `4`. The rejection
arm at each site is unchanged: `return None` in the blind walkers,
`veto_all(..)` in the scorer, `sink.malformed(..)` in the renderer.

`render_text/mod.rs:540-541` additionally recomputes `missing` as
described in S1.

### S3. An iterative group-extent scanner

A group's extent is found by walking tags until the `END_GROUP` that
matches its own field number. Both crates already have such a walker —
`any_field.rs:114` (`skip_group`) and `walk.rs:421`
(`parse_group_blind`) — and both **recurse** on `WT_START_GROUP`, which
is exactly the flaw in Background §2.

Both are rewritten **iteratively**, with a plain `usize` nesting counter
in place of the recursive call:

```rust
/// Offset just past the `END_GROUP` tag that closes the `START_GROUP`
/// whose own tag ends at `pos`, or `None` if the buffer runs out or a
/// tag along the way is unparsable.
///
/// `expected` is the opening tag's field number when the caller wants
/// the closing tag checked against it (`None` on a mismatch), or
/// `None` when the caller only wants the extent — see below.
///
/// Iterative on purpose. Matching group nesting needs a counter, not a
/// call stack, so this cannot overflow and needs no cap of its own —
/// which is what lets it be used *as* the recovery path for the render
/// recursion's cap (S4) rather than being subject to it.
fn scan_group_extent(buf: &[u8], pos: usize, expected: Option<u64>) -> Option<usize>
```

The body is the existing `skip_group` loop with the recursive
`WT_START_GROUP` arm replaced by `depth += 1`, and the `WT_END_GROUP`
arm by `depth -= 1`, returning when `depth` reaches zero.

Two consequences of dropping the call stack, both deliberate:

- **Inner closing tags are no longer checked against their openers.**
  The recursive form validated every level; matching all of them
  iteratively would need a `Vec<u64>` of open field numbers, an
  allocation in a routine that exists to be allocation-free. Only the
  outermost is checked, and only when `expected` is `Some`.
- **The depth-cap site passes `expected: None` and checks nothing at
  all.** Group nesting is determined by `START_GROUP`/`END_GROUP`
  pairing; the field number on the closing tag does not affect the
  extent, and the extent is all that site wants — it dumps the span
  verbatim. Checking would also be *stricter than the uncapped path*,
  which tolerates a mismatch and records `END_MISMATCH: n` in the close
  facts (`len_field.rs:305-309`); and a `None` there would fall back to
  `buflen` and swallow every following sibling, which is precisely what
  G3 forbids.

`skip_group`'s own caller keeps `expected: Some(field_number)`, so the
`Any`-scanning path behaves exactly as before at the outermost level.

`prototext-core`'s copy lives in a new
`render_text/helpers/group_scan.rs` rather than staying private to
`any_field.rs`, because S4 needs it from `render_text/mod.rs` too.
`prototext-graph`'s copy stays where it is (N1, N2), loses the `depth`
parameter an earlier draft of this spec gave it, and keeps its
mismatch-rejects behavior unchanged — turning that into a scoring
decision is spec 0172's business, not this one's.

`MAX_SCORE_DEPTH` is replaced by `MAX_WIRE_DEPTH` (same value, one
definition). `score_message_multi`'s own recursion cap stays — that one
is genuine recursion — and its rationale comment moves down from the
deleted constant to the check itself, gaining a note on why exceeding
*this* cap vetoes where the renderer's degrades locally: a range the
scorer cannot finish reading is a range it cannot honestly score, and
there is no partial verdict to fall back on.

### S4. Local depth cap on render recursion

**The counter.** A new thread-local beside the existing render-mode
cells (`render_text/mod.rs`):

```rust
thread_local! { static DEPTH: Cell<usize> = const { Cell::new(0) }; }
```

reset to `0` in `decode_and_render` and `decode_and_render_indexed` next
to `NODE_COUNT`, incremented and decremented by an RAII `DepthGuard`
taken at the top of `render_message` — the sole recursion hub, since
`render_len_field`, `render_group_field`, `render_any_expansion` and
`render_message_set_expansion` all recurse *through* it. A thread-local
rather than a parameter because a parameter would have to be plumbed
through four signatures already carrying
`#[allow(clippy::too_many_arguments)]` to reach one place; measured
cost is nil in the binaries, which are the only heavy-duty consumers
(local-exec TLS compiles to a single `mov` from `fs:`).

`DEPTH` is deliberately **shared** with `ProbeSink`, and this does not
violate `ProbeSink`'s "never mutates shared render-mode state"
invariant: `DEPTH` counts real stack frames, and the probe's frames sit
on top of the outer render's, so the outer depth is the correct
starting point — and the guard restores the previous value on the way
out, so nothing is disturbed.

**The decision sites.** The counter is only ever *consulted* at the two
places that recurse, via

```rust
#[inline]
fn at_depth_cap() -> bool { DEPTH.with(Cell::get) >= MAX_WIRE_DEPTH }
```

- **`render_len_field`, first statement.** When `at_depth_cap()`, render
  `data` as `ScalarValue::Bytes` and return. This is a single check that
  covers all four of that function's recursive branches — nested
  message, spec-0097 probe, `Any` expansion, `MessageSet` expansion —
  and it agrees with what the schemaless path already does at depth.
  The packed-array branch is caught by the same check and also degrades
  to bytes; that is a fidelity loss of one rendering shape, at depth
  1000, and is accepted rather than complicating the check.

- **`render_message`'s `WT_START_GROUP` arm.** When `at_depth_cap()`,
  do not call `render_group_field` at all:

  ```rust
  let end = scan_group_extent(buf, pos, None).unwrap_or(buflen);
  sink.malformed(0, TagFacts::default(), MalformedKind::InvalidTagType,
                 &buf[field_start..end]);
  pos = end;
  ```

  and let the loop continue. `field_start` is the group's own tag, so
  the dump spans tag through matching `END_GROUP` inclusive.

Both checks happen strictly before any `begin_nested`, which is what
keeps Background §4's write-in-place invariant intact.

**Why `InvalidTagType`.** No new grammar (G4). It is already
`render_message`'s own "I cannot parse further here" production
(`mod.rs:532-541`, for an unparsable wire tag), and it is the only
`MalformedKind` that re-encodes **tagless** — `encode_text/fields.rs:175`
writes the escaped bytes verbatim with no tag and no length prefix — so
a verbatim dump of `buf[field_start..end]` round-trips byte-for-byte.
The alternatives all re-emit a tag (`INVALID_LEN`, `INVALID_GROUP_END`,
`INVALID_VARINT`) or re-synthesize a length prefix
(`TRUNCATED_BYTES`), neither of which is correct for a group.

The cost is that the remainder is escaped and embedded in the output,
where `NODE_BUDGET_EXCEEDED` reports a length only. That is the same
exposure `mod.rs:532` already carries for a bad tag at offset 0, and it
is the price of not extending the grammar. It is also bounded by the
group's own extent, not by the rest of the buffer.

**The backstop.** `DepthGuard::enter()` returns `Option<DepthGuard>` and
`render_message` returns `(buflen, None)` after an `InvalidTagType` dump
of `buf[start..]` when it is `None`. With the two decision sites above
in place it is unreachable — the tests in the plan below all hit a
decision site, never this — and it exists only so that a *future*
recursion site added without a matching check degrades safely instead of
exhausting the stack. This is the one place the "abandon the buffer"
behavior Background §3 rejects still lives, which is acceptable for a
path that should never run.

**What is deliberately absent.** No new `MalformedKind` variant, no new
`render_*` helper in `helpers/scalar.rs`, no change to
`DecodeRenderOpts`. A capped render differs from an uncapped one only in
that one subtree is opaque.

## Test plan

All tests are `#[test]` in the crate that owns the code — no fixture
files, every input built inline.

**`prototext-core`**

- `len_prefix_near_u64_max_does_not_panic` — a LEN field whose length
  prefix is `u64::MAX` (ten bytes) renders as `TRUNCATED_BYTES` with the
  correct `MISSING` count. The Any and MessageSet scanners are not
  repeated here: both answer a bad bound with `None`, which merely
  declines the expansion and falls through to the ordinary render, so
  there is no distinct observable — `bounds.rs`'s own unit tests carry
  that case.
- `deeply_nested_len_does_not_overflow_the_stack` — 2 000 nested
  one-field LEN messages (twice the cap), schemaless, render without
  aborting.
- `deeply_nested_len_degrades_to_bytes_at_the_cap` — the same payload
  against the self-recursive schema `Node { optional Node child = 1; }`,
  so every level has a schema and `render_len_field` would recurse
  directly. Asserts exactly `MAX_WIRE_DEPTH - 1` open braces — the root
  `render_message` frame is depth 1 and each brace costs one more, so the
  last `render_len_field` free to recurse runs at `MAX_WIRE_DEPTH - 1` —
  and that the innermost rendered field is a bytes scalar, not a message.
  Before the fix this yields 2 000 open braces (2 000 frames does not
  actually overflow), so it fails cleanly rather than by crashing.
- `over_deep_group_costs_only_itself` — the proving test for G3, and the
  only one that exercises `scan_group_extent` from the render path. A
  message holding, in order: a scalar field, one group nested 1 200
  levels deep and *properly closed*, then a second scalar field. Asserts
  both scalars render normally and the group renders as a single
  `INVALID_TAG_TYPE` line. The trailing scalar is the whole point: the
  naive "abandon the buffer" cap loses it.
- `capped_render_still_round_trips` — the same payload re-encoded
  through `encode_text` is byte-identical to the input. This is what
  pins the choice of `InvalidTagType` (G4); any tag-emitting variant
  fails here.
- `deeply_nested_unterminated_groups_do_not_overflow_the_stack` —
  200 000 `START_GROUP` tags with no matching ends, so
  `scan_group_extent` returns `None` and the `unwrap_or(buflen)` arm is
  taken.
- `tripping_the_depth_cap_does_not_leak_the_counter` — `DEPTH` is a
  thread-local and protolens reuses render threads, so a guard that
  failed to unwind would silently cap every later render on the same
  thread. Not a red-before test (the counter did not exist); it exists to
  keep the RAII discipline from rotting.
- `over_deep_group_with_a_mismatched_close_still_costs_only_itself` —
  the same shape as `over_deep_group_costs_only_itself` but with the
  outermost `END_GROUP` naming a different field number. Must be
  indistinguishable in outcome: `expected: None` at that site means the
  trailing scalar still renders. This is the test that would fail if
  someone reintroduced the check.
- Unit tests on `scan_group_extent`: exact extent for a well-formed
  nested group; `None` on truncation; `None` on a garbled inner tag;
  `None` for a mismatched outermost close when `expected` is `Some`;
  `Some` for the same input when `expected` is `None`; and — pinning the
  S3 relaxation deliberately — `Some` for an *inner* `END_GROUP` whose
  field number does not match its opener, either way.
- Regression: with nesting under the cap, every existing test output is
  unchanged, byte for byte. In particular the depth cap must be
  invisible to `node_budget_truncates_deep_nesting_with_a_visible_marker`
  and to the fixture-driven `descriptor.pb` renders.

**`prototext-graph`**

- `len_prefix_near_u64_max_vetoes_rather_than_panicking` — `score_all`
  over the same hostile buffer returns with the candidate vetoed rather
  than panicking on `&buf[pos..pos + length]`.
- `blind_group_walk_does_not_overflow_the_stack` — 200 000
  `START_GROUP` bytes with no matching ends, scored against a graph in
  which the field is unknown, returns rather than overflowing.
- Regression: `hopcroft_suite` and the existing `score` tests are
  unaffected.
