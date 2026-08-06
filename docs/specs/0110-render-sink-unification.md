<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0110 — `render_message`/`parse_message` unification via a `Sink` abstraction

**Status:** implemented
**Implemented in:** 2026-07-09
**Refs:** `docs/specs/0097-raw-recursive-lendel.md`, `docs/prototext/design.md`,
`docs/prototext/PROST-ISSUES.md`, `docs/prototext/protoc-decode-compatibility.md`,
`docs/specs/0111-protolens-v1-decode-navigate-extract.md`
**App:** prototext-core

---

## Background

`prototext-core` currently contains two independently-maintained implementations of
"walk a protobuf's wire bytes field by field":

1. `decoder::parse_message`/`ingest_pb` — builds a typed tree (`ProtoTextMessage` of
   `ProtoTextField`s). Public API (`pub use types::*` re-exports the tree types; an
   external test in `prototext/tests/roundtrip.rs` calls it directly).
2. `render_text::decode_and_render`/`render_message` — a separate, single-pass
   streaming decoder that formats and writes text directly as it walks the wire
   bytes, discarding structural/position information as it goes. This is what
   `prototext decode`'s CLI actually calls; `decoder::parse_message`/`ingest_pb` is
   never referenced anywhere in `prototext/src`.

Both share only the low-level wire-primitive helpers (`parse_varint`,
`parse_wiretag`, `decode_fixed32`, ...); the per-field dispatch/orchestration logic
is reimplemented independently in each — confirmed by two separate, non-shared
`packed.rs` files (`decoder/packed.rs`, 227 lines; `render_text/packed.rs`, 361
lines). This duplication has already produced one confirmed instance of drift:
`docs/prototext/design.md` claims the binary round-trip path (`render_as_bytes`) depends on
`ingest_pb`'s IR — verified false; `serialize/encode_text/mod.rs` uses its own
independent `Frame`/placeholder-compaction machinery and never references
`decoder::ingest_pb`/`ProtoTextMessage` at all.

`decoder::parse_message`'s actual footprint today is narrow. Exhaustive repo-wide
search finds:

- **One production call site**: `render_text/helpers/len_field.rs:51`, and its
  mirror inside `decoder/packed.rs::decode_len_field` (called when the field being
  probed is itself unknown-typed and nested inside another probe) — both used
  purely as a disposable structural-validity probe for spec 0097's unknown-LEN-field
  three-step cascade (message / string / bytes). The resulting `ProtoTextMessage`
  tree is discarded immediately; only `malformities == 0 && next_pos == data.len()`
  is read.
- **One direct unit test**: `roundtrip.rs::len_wire_type_on_varint_field_sets_type_mismatch_flag`,
  covering one narrow regression (a LEN wire type on a declared-`int32` field must
  set `proto2_has_type_mismatch`).
- **One dead downstream consumer**: `serialize/common/format.rs::format_protoc_value`,
  which formats a single `ProtoTextField`'s value — zero callers anywhere in the
  crate.

Replacing `decoder::parse_message` with a leaner structural probe is motivated on
its own, independent of any future consumer: `len_field.rs:51`'s probe call site
exists today, and today's implementation has a confirmed gap — a nested group's
own malformity count is discarded rather than rolled up into the caller's
(`decoder/mod.rs`'s `WT_START_GROUP` arm binds it to `_`). A probe that is both
leaner (no tree-building, no scalar-value decoding, no nested-LEN classification)
and more correct (group malformities rolled up) is worth building regardless of
whether `protolens` ever ships.

A planned future consumer (spec 0111, `protolens` v1) additionally needs a
byte-offset index over the *same* rendering `render_text`/`decode_and_render`
already performs — pairing each node with its raw byte range in the source
protobuf, its range in the rendered text, and its indentation level.

Building the probe and the index as yet more independently-maintained decode
loops would only deepen the crate's existing divergence risk. Instead, this spec
unifies decode/render/probe/index behind one generic `Sink` abstraction,
monomorphized at compile time for zero runtime cost in each specialization, so
there is exactly one body of per-field dispatch logic serving all three needs —
with `decoder::parse_message`/`ingest_pb` and its supporting types retired once
the migration lands. This spec is self-contained and does not depend on spec 0111
for its own implementation; it is written to also satisfy 0111's stated needs,
which is a hard prerequisite in the other direction (0111 cannot start until this
spec is implemented).

---

## Goals

1. Define a `Sink` trait capturing the semantic events `render_message`'s dispatch
   loop already produces at each field — scalar field decoded, begin/end nested
   message or group, malformed field — at a level carrying enough information
   (field number, raw byte offset range) that each planned implementation can do
   its own thing without re-deriving anything the dispatch loop already knows.
2. Refactor `render_message`'s internals, and its helpers across
   `len_field.rs`/`packed.rs` (render_text)/`scalar.rs`/`annotations.rs`, to route
   output exclusively through `Sink` methods. This includes
   `any_field.rs::render_any_expansion` and
   `message_set_field.rs::render_message_set_expansion`: both currently write
   their synthetic wrapper lines (`type_url: "..."`, `value {`, `Item {`,
   `type_id: N`, `message {`) directly, outside any per-field dispatch (see
   Design rationale) — these become `Sink`-generic too, so the wrapper node
   itself (not just its recursively-rendered children) is visible to
   `IndexingTextSink`. **Zero behavior change** to today's production text
   output — byte-for-byte identical, verified by the existing `render_text`
   test suite (no new/changed expected-output fixtures).
3. Implement three `Sink`s:
   - **`TextSink`** — today's production behavior, used by `decode_and_render`/
     `prototext decode`.
   - **`ProbeSink`** — a lean, no-text-output structural-validity walk: mandatory
     recursion into groups (rolling their malformity count into the caller's own),
     no recursion into LEN payload *contents* (skip by declared length only —
     "shallow", per §2). The caller reads `next_pos` from `render_message`'s own
     return value and the total malformity count via
     `ProbeSink::malformity_count() -> u32`, read off the sink after the call
     returns (§1). Replaces the `len_field.rs:51`/`decoder/packed.rs` probe call
     sites.
   - **`IndexingTextSink`** — wraps a `TextSink`, additionally recording one
     `NodeSpan` (`raw_range`, `text_range`, `level`, `type_fqdn`) per
     field/node into a side index (§3), nested by construction (a child's
     ranges always sit inside its parent's, in both dimensions). `text_range`
     is a **line-number range**, not a byte range (see Design rationale) —
     read directly off `TextSink`'s own `line_count` counter, never
     independently derived. Thanks to Goal 2's `any_field.rs`/
     `message_set_field.rs` refactor, `Any`/`MessageSet`-expanded wrapper
     nodes get real `NodeSpan` entries too, not just their recursively-
     rendered children. Exposed via a new public entry point,
     `decode_and_render_indexed` (§3), returning `(Vec<u8>, Vec<NodeSpan>)`.
     This spec defines and implements the index; it has no consumer of its
     own — `protolens` v1 (spec 0111) is the first.
4. Add an initial-indentation-level parameter (and header-suppression) to
   `decode_and_render`, so a freshly re-rendered sub-slice can come out already
   indented to the depth at which a future caller will splice it back into a
   composite view, with no post-hoc text patching (§4). No caller of this
   parameter exists yet within this spec's own scope — it exists to satisfy a
   need spelled out by spec 0111's design (and, further out, spec 0109's
   override-and-splice mechanism).
5. Retire `decoder::parse_message`/`ingest_pb`, `ProtoTextMessage`/`ProtoTextField`/
   `ProtoTextContent`, `decoder/packed.rs`, and the dead
   `serialize/common/format.rs::format_protoc_value`, once `ProbeSink` covers the
   one real production use case. Replace the one direct test in `roundtrip.rs`
   with a `ProbeSink`-based equivalent, preserving the same regression coverage
   (§5).

## Non-goals

- Any change to `render_text`'s actual formatting output. This refactor must be
  behavior-preserving for `TextSink` — this is not an opportunity to also revise
  annotation formatting, escaping, or indentation rules.
- Rolling LEN-payload malformities into a probe's count (see §2) — a deliberate,
  narrower scope than a hypothetical "fully recursive" probe.
- Building `protolens` itself, or any consumer of `IndexingTextSink`'s index or
  `decode_and_render`'s new parameters — that is spec 0111 and beyond.
- Implementing the override-and-splice mechanism the index/initial-level design
  anticipates (Design rationale, below) — deferred to the spec that introduces
  interactive override, per spec 0109.
- Removing or repurposing `MESSAGE_SET_NODE_ID`/the dead
  `google.protobuf.MessageSet` reservation in `graph.rs`/`hopcroft.rs` — unrelated,
  spec 0108's existing non-goal, untouched here.
- Extending `ProbeSink` to schema-aware contexts so it can also resolve
  `Any`/`MessageSet` expansion during a structural probe. `ProbeSink`'s sole
  invocation site (Goal 3, §2) is `render_len_field`'s `field_schema: None`
  branch — genuinely schemaless data; `Any`/`MessageSet` expansion only ever
  runs in the opposite branch (`field_schema: Some(fs)`, with `fs.kind()`
  already resolved), so `ProbeSink` structurally never reaches this code by
  construction, not merely by choice. `ProbeSink`'s sole purpose — given
  schema-less bytes, determine whether they could possibly be interpreted as
  a message at all — is orthogonal to resolved-type expansion.

---

## Design rationale: why the index carries `raw_range` + `text_range` + `level` together

`NodeSpan` (§3) captures three fields even though this spec has no consumer that
uses all three yet, specifically to avoid a second index migration once a future
override-and-splice consumer (spec 0111's own follow-up, and spec 0109's fuller
design) lands. The envisioned mechanism such a consumer would use:

1. Extract a node's raw bytes via `raw_range`.
2. Re-decode them via `decode_and_render(sub_bytes, candidate_type,
   initial_level = node.level, ...)` (§4) — producing text already indented to
   match the depth at which it will be spliced in, with no post-hoc reindentation.
3. Convert `node.text_range`'s line-number bounds to byte offsets into `out` (a
   cheap, on-demand scan — see Design rationale) and replace that byte range with
   the fresh text (the same literal `Vec::splice` technique `render_group_field`
   already uses internally for its own close-tag backtracking — not new
   machinery, applied one level up).
4. Re-run `IndexingTextSink` over just the extracted sub-slice under the new type,
   and use its fresh index entries to replace the stale ones for that subtree —
   everything outside the replaced subtree's `text_range`/`raw_range` stays valid
   untouched, consistent with spec 0109's path-copying undo model.

Two invariants make this concrete and unambiguous, both confirmed against the
actual codebase rather than assumed:

- **Nesting.** A child's `raw_range` and `text_range` both nest strictly inside its
  parent's, and both are absolute/flat — a single coordinate space spanning the
  whole document, not local to each node's immediate parent. For `raw_range`:
  children are always *parsed* from a sub-slice of the parent's own bytes (a LEN
  field's `data`, or the portion of `buf` strictly between a group's start and
  end tags) — recursion only ever narrows the byte window `render_message` itself
  sees. But `IndexingTextSink` does not store that local window directly: `Sink`'s
  `begin_nested`/`begin_virtual_nested` also carry a `payload_start` — the local
  offset, in the *same* frame as the node's own `raw_start`, at which its
  recursively-rendered content begins (`0` for `NestedKind::Group`, whose
  children stay in the *same* buffer as the group's own tag — no length prefix,
  hence no new frame; the LEN-delimited payload start otherwise). `IndexingTextSink`
  accumulates these into a `raw_base` — the absolute offset local `0` currently
  maps to — carried per open node in `IndexMark` and pushed/popped exactly like
  `LEVEL`, so every `NodeSpan::raw_range` it stores is already translated to
  absolute coordinates before a consumer ever sees it. `TextSink`/`ProbeSink`
  ignore `payload_start` entirely — the translation work is paid for once, inside
  `IndexingTextSink`'s own (non-hot-path) bookkeeping, not by every downstream
  consumer of `Vec<NodeSpan>`. For `text_range`: the parent writes its opening line *before* recursing
  and its closing line *after* (`render_group_field`'s "greedy write, splice
  post-hoc" pattern; `render_len_field`'s `wob_prefix_n` before /
  `write_close_brace` after), so every line of every descendant's text
  necessarily falls between those two lines.
- **One field per rendered line, no wrapping.** Confirmed via
  `docs/prototext/protoc-decode-compatibility.md:30` ("Multi-line mode (default) — one field
  per line, 2-space indentation") — `prototext decode`'s canonical output
  deliberately mirrors `protoc --decode`'s own convention and never wraps a value
  across multiple physical lines, regardless of length. So a node's boundaries
  always fall exactly on line boundaries — there is never a sub-line position to
  represent. (Any *visual* wrapping of an overly long single line, for display
  within a fixed terminal width, is a downstream TUI presentation concern layered
  on top — orthogonal to this index, not addressed here.)

**`text_range` is stored as a line-number range, not a byte range — this is a
correctness requirement, not just a convenience choice.** `render_group_field`'s
existing post-hoc splice (`len_field.rs:289-444`) inserts annotation text — the
field's own `field_decl`, tag-mismatch/close-tag modifiers (`OPEN_GROUP`,
`etag_ohb: N`, `ETAG_OOR`, `END_MISMATCH: N`, etc.), all joined by `"; "` — at
`header_nl_pos`, the byte position of the group's *own* header-line newline,
captured *before* recursing into the group's children (`len_field.rs:352`). This
insertion always lengthens the header line itself; the code confirms it never
contains a literal `\n` — it is built purely from single-line annotation
fragments (`len_field.rs:409-428`). Consequently:

- Every byte position recorded *during* the walk, for anything at or after
  `header_nl_pos` — which includes every already-recorded span for the group's own
  descendants, since they were all parsed and recorded *before* this post-hoc
  splice runs — becomes stale by exactly `+n` (`n` = the inserted annotation's
  byte length) the moment the splice executes. The codebase already performs this
  exact correction for the one byte-position value it tracks today:
  `CBL_START.with(|c| c.set(c.get() + n))` (`len_field.rs:440`). A byte-based
  `IndexingTextSink` would need the same `+n` correction applied to every
  already-recorded descendant span, for every group in the input — a real
  compaction pass, required on the very *first* render pass, not only for some
  hypothetical future override-splice feature.
- A line-based `text_range` needs **no correction at all** for this splice,
  because the splice never writes a `\n`.

This is not a coincidence of today's specific annotation strings — it reflects a
structural distinction the format already draws: `\n` marks "a field's rendering
begins here" (sibling or nested), while post-hoc splicing exists *only* to attach
facts about a field whose line is already open (its own tag-mismatch/close-tag
modifiers) — never to introduce another field. To make this a construction
guarantee rather than an assumption to re-verify by inspection whenever
annotation formatting changes, `TextSink` centralizes every `\n` write behind one
method:

```rust
impl TextSink {
    fn newline(&mut self) {
        self.out.push(b'\n');
        self.line_count += 1;
    }
}
```

replacing all 22 of today's scattered `out.push(b'\n')` call sites (across
`scalar.rs`, `output.rs`, `any_field.rs`, `message_set_field.rs`, `len_field.rs`,
`varint.rs`, `packed.rs`). Post-hoc splice code (`render_group_field`'s fixup
block) builds its `insert` string via `String::push_str`/`out.splice` and never
calls `newline()` — so `line_count` is provably unaffected by any post-hoc
splice, regardless of what future annotation content gets added to it.
`IndexingTextSink` reads `text_sink.line_count` directly at each `Sink` event —
no independent delta-tracking, no risk of drift, no special-casing needed for the
group-splice path.

Converting a `text_range` line number back to a byte offset into the physical
`Vec<u8>` output buffer — needed only when an actual override-splice happens, a
rare, user-triggered action, not a per-node or per-field operation — is a cheap,
on-demand scan (count `\n` bytes up to the target line), deliberately *not*
tracked eagerly per node.

The splice-based backtracking pattern itself remains warranted and is *not*
removed by this design — it exists because `TextSink` writes to an append-only
byte stream, and a group's close-tag facts (open-ended? mismatched end field?
close-tag overhang?) are only known after recursing into it. `IndexingTextSink`
resolves the exact same "facts known only post-recursion" problem without any
byte-splicing at all, because its index entries are plain struct fields /
`Vec` entries, not positions in a serialized byte stream — assigned post-hoc in
any order, exactly as `decoder::parse_message`'s existing `WT_START_GROUP` arm
already does today (recurse, then assign `open_ended_group`/
`end_tag_overhang_count`/`mismatched_group_end` onto `field`, no splicing
needed).

### Any/MessageSet expansion under the Sink model

`any_field.rs::render_any_expansion` and
`message_set_field.rs::render_message_set_expansion` today follow the same
pattern: a bespoke, non-generic scan (`scan_any_fields`, `scan_message_set`)
extracts the sub-slices it needs (`fields.value`; each item's `type_id`/
`message`), then the synthetic wrapper lines (`type_url: "..."`, `value {`,
`Item {`, `type_id: N`, `message {`) are written by hand — direct
`push_indent`/`out.extend_from_slice` calls, outside any per-field dispatch —
and only the final, extracted sub-message is rendered through a genuine
recursive `render_message` call. Left as-is, migrating `render_message` to
`Sink` would leave these hand-written wrapper lines invisible to
`IndexingTextSink`: the `value {}` / `message {}` node — exactly the node
`protolens` most needs to select for override, per spec 0109 — would get no
`NodeSpan` at all, only its recursively-rendered children would.

Goal 2 closes this gap by threading `sink: &mut S` into both expansion
functions and replacing each hand-written line with `virtual_scalar`/
`begin_virtual_nested`/`end_nested` calls — dedicated methods (§1) rather
than `scalar_field`/`begin_nested`, since these wrapper lines are not backed
by any real `FieldOrExt` schema descriptor (`type_url`, `type_id`, `value {`,
`Item {`, `message {` are hand-authored literal strings, not a declared
field's schema-driven rendering) and so cannot flow through
`scalar_field`/`begin_nested`'s schema-driven signature. `raw_range` is
computed directly from the scan's already-known sub-slice offsets (no new
byte-scanning). Under `TextSink`, this is a pure refactor with no output
change, since `virtual_scalar`/`begin_virtual_nested`/`end_nested` compile
down to the exact same `push_indent`/`extend_from_slice` sequence they
replace. Under `IndexingTextSink`, the wrapper node now gets a real
`NodeSpan`, with
`type_fqdn` set to the *resolved* type (from `type_url`, or from the
`(extendee_fqdn, type_id)` extension lookup) — which is generally different
from the field's own declared type, and is exactly the information spec
0109's status line ("currently assigned type") needs to display. `ProbeSink`
is unaffected and never reaches this code at all (see Non-goals).

---

## Specification

### §1 — `Sink` trait shape

Illustrative sketch; exact signatures are expected to be refined during
implementation, but the shape/responsibilities below are intended to hold:

```rust
pub(crate) trait Sink {
    /// Per-implementation "in-progress nested node" marker, returned by
    /// `begin_nested`/`begin_virtual_nested` and passed back to `end_nested`.
    /// `TextSink` uses this to remember the byte position of its
    /// greedily-written opening line, plus whatever post-hoc splice content
    /// (field_decl, mismatch modifier) it will need once the group's close
    /// facts are known — exactly the local variables `render_group_field`
    /// already computes today, just relocated onto this struct. `ProbeSink`
    /// uses `()`. `IndexingTextSink` uses it to remember which index-entry
    /// slot to backfill.
    type Mark;

    /// A scalar (non-recursive), schema-backed field has been fully parsed
    /// off the wire. `value` carries the wire-kind-specific raw payload —
    /// enough for `TextSink`'s own implementation to reproduce today's exact
    /// typed formatting (decode, escape, annotate; `is_mismatch`/`unknown`/
    /// NaN-bits/enum-name are all recomputed from `field_schema` + `value`
    /// inside that implementation, not passed in separately). `ProbeSink`'s
    /// implementation never inspects `value` at all: reaching this call
    /// already means the field parsed validly off the wire, which is
    /// everything a shallow probe needs to know.
    #[allow(clippy::too_many_arguments)]
    fn scalar_field(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        value: ScalarValue<'_>,
        raw_range: Range<usize>,
        schema_present: bool,
    );

    /// A nested message or group is about to be parsed. `kind` distinguishes
    /// a LEN-delimited nested message from a GROUP wire record — `TextSink`
    /// needs this to pick the right opening-line convention (`wob_prefix_n`
    /// vs. `render_group_field`'s greedy-write-then-splice path).
    fn begin_nested(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        kind: NestedKind,
        raw_start: usize,
    ) -> Self::Mark;

    /// The nested message/group finished. `raw_range` is its full byte span in
    /// the source buffer. `close_facts` is `Some` only for groups (facts only
    /// knowable post-recursion: open-ended, mismatched end field, close-tag
    /// overhang) — `None` for LEN-delimited nested messages, which have no
    /// separate close tag.
    fn end_nested(
        &mut self,
        mark: Self::Mark,
        raw_range: Range<usize>,
        close_facts: Option<GroupCloseFacts>,
    );

    /// A synthetic "virtual field" scalar line — used only by the Any/
    /// MessageSet expansion wrappers (Design rationale) for lines
    /// (`type_url: "..."`, `type_id: N`) that are not backed by any real
    /// schema field descriptor, so `scalar_field`'s `field_schema`-driven
    /// formatting doesn't apply. `name`/`annotation` are already fully
    /// formatted by the caller (`any_field.rs`/`message_set_field.rs` keep
    /// authoring these exact literal strings, unchanged from today).
    fn virtual_scalar(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        value_str: &str,
        raw_range: Range<usize>,
    );

    /// A synthetic "virtual field" nested-node opener — used only by the
    /// Any/MessageSet wrappers (`value {`, `Item {`, `message {`). Always
    /// paired with `end_nested(mark, raw_range, None)` — a virtual nested
    /// node is never a group, so `close_facts` is always `None`.
    fn begin_virtual_nested(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        raw_start: usize,
    ) -> Self::Mark;

    /// A structurally invalid field was encountered at the current level.
    /// `field_number` is `0` for `MalformedKind::InvalidTagType` (the one
    /// case where no field number could even be parsed — mirrors today's
    /// `render_invalid_tag_type`/`wfl_prefix("0", out)`).
    fn malformed(&mut self, field_number: u64, tag: TagFacts, kind: MalformedKind, raw: &[u8]);

    /// Whether `render_len_field` should treat every LEN-delimited field as
    /// opaque bytes — skipping its unknown-field cascade (nested-message
    /// probe, packed detection, Any/MessageSet expansion) entirely — rather
    /// than recursing into it (§2). `ProbeSink` overrides this to `true`: a
    /// probe only ever needs mandatory recursion into GROUPs (which have no
    /// length prefix, so their extent is unknowable without parsing through
    /// them); a LEN field's own length prefix, already bounds-checked by
    /// `render_message` before dispatch, is sufficient on its own. Default
    /// `false` for every other `Sink`. Discovered during implementation: §2's
    /// original "shallow, no recursive classification" prose for `ProbeSink`
    /// conflicted with `render_len_field`'s literal shared-cascade structure
    /// (which, under `schema: None`, always performs a recursive Step-1
    /// nested-message probe regardless of which `Sink` is active) — this hook
    /// is the resolution, confirmed with the spec's author.
    fn treat_len_as_opaque(&self) -> bool {
        false
    }

    /// Whether this sink's own rendering depends on `LEVEL`, the shared
    /// thread-local recursion-depth counter used for indentation (`enter_level`
    /// consults this before touching `LEVEL` at all). `ProbeSink` overrides
    /// this to `false`: it never indents anything (all its methods are
    /// no-ops), so it must not mutate `LEVEL` on behalf of the in-progress
    /// outer render (typically a `TextSink` pass) that invoked it as a
    /// read-only structural probe. Default `true` for every other `Sink`.
    /// Discovered during implementation: `ProbeSink` is meant to be a
    /// read-only helper and must not touch any shared render-mode state
    /// belonging to the render pass that invoked it, even transiently.
    fn tracks_level(&self) -> bool {
        true
    }
}

/// Tag-level anomaly facts shared by nearly every dispatch site — mirrors the
/// `tag_ohb`/`tag_oor`/`len_ohb` triple already threaded through nearly every
/// `render_*` function today. `len_ohb` is only ever populated for LEN-wire
/// fields (`None` for VARINT/FIXED32/FIXED64 — the same always-`None` pattern
/// already present in today's `ScalarCtx`).
#[derive(Clone, Copy, Default)]
pub(crate) struct TagFacts {
    pub tag_ohb: Option<u64>,
    pub tag_oor: bool,
    pub len_ohb: Option<u64>,
}

/// Wire-kind-specific raw payload for a scalar field. Each variant carries
/// exactly what today's corresponding `render_*` function already takes as
/// input, before any schema-typed decoding — that decoding stays inside
/// `TextSink`'s own `scalar_field` implementation, never in the shared
/// dispatch loop.
pub(crate) enum ScalarValue<'a> {
    /// VARINT wire type. `raw_val` is `parse_varint`'s own decoded output;
    /// `val_ohb` is the value varint's own overhang count.
    Varint { raw_val: u64, val_ohb: Option<u64> },
    /// FIXED64 wire type: the raw 8 bytes.
    Fixed64([u8; 8]),
    /// FIXED32 wire type: the raw 4 bytes.
    Fixed32([u8; 4]),
    /// LEN wire type, non-packed: string, bytes, or wire-type-mismatch bytes
    /// leaf (`TextSink` recomputes which, from `field_schema`/`kind`, exactly
    /// as today's `render_len_field` already does).
    Bytes(&'a [u8]),
    /// LEN wire type, packed-repeated: the whole wire record. `TextSink`
    /// decodes and emits one line per element (today's `render_packed`);
    /// `IndexingTextSink` still records exactly one `NodeSpan` per call — its
    /// `raw_range` covers the whole record, its `text_range` the full run of
    /// emitted element lines (§3).
    Packed(&'a [u8]),
}

/// Distinguishes a LEN-delimited nested message from a GROUP wire record at
/// `begin_nested` — see that method's doc comment.
pub(crate) enum NestedKind {
    Message,
    Group,
}

/// Discriminates the specific structurally-invalid case `malformed` reports,
/// collapsing today's five separate `render_invalid*`/`render_truncated_bytes`
/// call shapes into one event with one payload shape.
pub(crate) enum MalformedKind {
    InvalidTagType,
    InvalidVarint,
    InvalidFixed64,
    InvalidFixed32,
    InvalidLen,
    TruncatedBytes { missing: u64 },
    InvalidGroupEnd,
}

/// Facts about a group's own closing tag, only knowable *after* recursing
/// into the group and reaching its `END_GROUP` tag (or running out of
/// buffer) — never applicable to a LEN-delimited nested message, which has
/// no separate close tag at all. Mirrors, field for field, what
/// `decoder::parse_message`'s `WT_START_GROUP` arm already computes today
/// (`decoder/mod.rs`; `docs/prototext/design.md`) — minus `open_ended_group`, which is
/// redundant here: `end_nested`'s `close_facts: Option<GroupCloseFacts>`
/// already encodes "open-ended" via its `None` variant, so a second boolean
/// saying the same thing would be dead weight (confirmed during
/// implementation — `cargo build`'s `dead_code` lint flagged the field as
/// never read, since the only construction site always passed `false`).
pub(crate) struct GroupCloseFacts {
    /// The `END_GROUP` tag's own varint was over-encoded (more bytes than
    /// the minimal encoding), and by how many.
    pub end_tag_overhang_count: Option<u64>,
    /// The `END_GROUP` tag's own field number was itself out-of-range
    /// (mirrors `wfield_oor` on the open tag) — reported as `ETAG_OOR`.
    /// Discovered during implementation: needed alongside
    /// `end_tag_overhang_count`/`mismatched_group_end` to reproduce today's
    /// `render_group_field` output byte-for-byte (a fourth close-tag fact,
    /// missing from this struct's original draft).
    pub end_tag_is_out_of_range: bool,
    /// The `END_GROUP` tag's field number didn't match the `START_GROUP`'s —
    /// structurally inconsistent, but non-fatal (parsing continues); carries
    /// the actual (mismatched) field number found.
    pub mismatched_group_end: Option<u64>,
}
```

`render_message<S: Sink>(buf, start, my_group, schema, schema_present, sink: &mut S)
-> (usize, Option<WiretagResult>)` becomes the one generic body; control flow (tag
dispatch, match arms, recursion) is unchanged from today — only the leaf-level
"append formatted bytes"/"increment malformities" calls are redirected through
`sink`. `render_message`'s own return value only ever carries `next_pos` and
`Option<WiretagResult>` — structural facts every caller needs regardless of
which `Sink` is in use. Anything a specific `Sink` implementation accumulates
for its own purposes (`TextSink::line_count()`, `ProbeSink::malformity_count()`,
§2) is read directly off that `Sink` after the call returns, not threaded
through `render_message`'s generic return type.

Because every level of recursion is threaded the *same* `&mut S` — never a
fresh sink per level, never a return-value hand-off — a nested group's
malformities are rolled into the caller's total automatically, by
construction: there is no separate "roll up the count" step to add or forget.
This structurally forecloses the exact bug `decoder::parse_message`'s
`WT_START_GROUP` arm has today (Background; discarding the nested call's
malformity count by binding it to `_`) — under the `Sink` model there is no
return value to discard in the first place.

`TextSink` additionally needs raw byte-append, splice-before-position, and a
`newline()` method (§ Design rationale — the sole writer of `\n` into `out`,
maintaining `line_count`) to do its actual formatting work. These are
**private, inherent methods on the `TextSink` struct — not a trait, not even a
`TextSink`-only extension trait**: nothing outside `TextSink`'s own `Sink`-method
implementations ever calls them. `ProbeSink` doesn't format text at all.
`IndexingTextSink` wraps a *concrete* `TextSink` by composition, not a generic
`S: Sink`, so it only ever needs read access to one counter —
`TextSink::line_count(&self) -> usize` — not the write methods themselves.

### §2 — `ProbeSink` semantics

Replaces `decoder::parse_message`'s role at the spec-0097 cascade sites, but
narrower in scope than what `decoder::parse_message` computes today:

- **VARINT/FIXED32/FIXED64 fields**: validate the value fits in the remaining
  buffer; no value decoding. `malformed()` on truncation/invalid varint.
- **LEN fields**: validate the length varint and that `pos + length <= buflen`;
  **skip over the payload by its declared length without inspecting its
  contents** — no recursive message/string/bytes classification of nested LEN
  payloads (the "shallow" mode this spec adopts). A LEN field's own malformed
  *contents* never affect the enclosing probe's malformity count.
- **Groups**: mandatory full recursion (unlike LEN fields — a group has no
  length prefix; its extent can only be determined by parsing through it to find
  its `END_GROUP` tag). The recursive call's own malformities **are** rolled up
  into the caller's count.

This last point is a deliberate behavior difference from today's
`decoder::parse_message`, which currently discards a nested group's own
malformity count (`WT_START_GROUP`'s arm binds it to `_`). `ProbeSink` rolling
group malformities up is considered more correct (a probe should not accept a
LEN payload as "cleanly a message" if a group nested inside it was itself
malformed) — flagged explicitly as an open issue (§ Open Issues) since it is a
behavior change, not a pure refactor, and needs test coverage confirming it
doesn't regress spec 0097's existing cascade test fixtures.

The `len_field.rs:51`/`decoder/packed.rs` call sites use `ProbeSink` as:
`let mut probe = ProbeSink::default(); let (next_pos, _) =
render_message(data, 0, None, schema, false, &mut probe); let ok = probe.malformity_count() == 0 && next_pos == data.len();`
— replacing today's `malformities == 0 && next_pos == data.len()` check
against `decoder::parse_message`'s returned tuple (Background) with the same
shape, read off the sink instead of a return value (§1).

### §3 — `IndexingTextSink` / node-span index shape

```rust
pub struct NodeSpan {
    pub field_number: u64,
    pub raw_range: Range<usize>,   // byte range in the source protobuf, absolute
                                    // w.r.t. the original top-level `buf` — not
                                    // local to this node's immediate parent
                                    // (Design rationale, "Nesting")
    pub text_range: Range<usize>,  // line-number range in the rendered text
    pub level: usize,              // indentation depth (matches render_text::LEVEL)
    pub type_fqdn: Option<String>, // FQDN of the type this node was rendered as,
                                    // when known: the declared field type, or —
                                    // for an Any/MessageSet-expanded wrapper node
                                    // (Design rationale) — the *resolved* type,
                                    // which generally differs from the field's own
                                    // declared type. `None` for scalar fields and
                                    // any node whose type genuinely isn't known
                                    // (e.g. a `ProbeSink`-only context has no
                                    // `IndexingTextSink` at all, so this never
                                    // arises there).
}
```

`IndexingTextSink` wraps a `TextSink`, delegating every `Sink` call to it
unchanged (so its text output is byte-for-byte identical to plain `TextSink`),
and additionally appends one `NodeSpan` per `scalar_field`/`end_nested` call into
a `Vec<NodeSpan>`, reading `text_range` directly off `TextSink`'s own
`line_count` (§ Design rationale) — never independently derived, and
unaffected by `render_group_field`'s post-hoc splice. Thanks to the
Any/MessageSet refactor (Design rationale), a `value {}`/`message {}` wrapper
node emits its own `NodeSpan` with `type_fqdn` set to the resolved type, not
just its recursively-rendered children. The nesting invariant
(Design rationale, above) guarantees this `Vec`, read in emission order, is a
valid pre-order flattening of the node tree — parent/child/sibling relationships
can be recovered from `raw_range`/`text_range` containment without needing an
explicit parent pointer, though an explicit `parent_index: Option<usize>` field
may be added during implementation if pre-order-flattening reconstruction proves
inconvenient for a downstream consumer (spec 0111 flags this as an open question
on its own side).

**Public entry point.**

```rust
pub fn decode_and_render_indexed(
    buf: &[u8],
    root_desc: Option<&MessageDescriptor>,
    annotations: bool,
    indent_size: usize,
    expand_any: bool,
    hide_unknown_fields: bool,
    expand_message_set: bool,
    initial_level: usize,
    emit_header: bool,
) -> (Vec<u8>, Vec<NodeSpan>)
```

A sibling entry point to `decode_and_render` (§4), sharing the exact same
parameter list, but internally constructing an `IndexingTextSink` instead of a
bare `TextSink`, and returning both the rendered text and its `NodeSpan`
index. This is the function spec 0111's `decode.rs` calls (spec 0111 §1:
"call `decode_and_render` with an `IndexingTextSink`, hand back `(text,
Vec<NodeSpan>)`" — that text refers to this function). `decode_and_render`
itself (§4) stays `TextSink`-only and keeps its existing `Vec<u8>`-only return
type: its production callers (`prototext` CLI, `prototext-pyo3`) have no use
for the index and shouldn't pay even `IndexingTextSink`'s small extra
bookkeeping cost (Open Issue #3).

**Tests.** `IndexingTextSink` has no consumer within this spec's own scope
(Goal 3), so its correctness must be established by direct unit tests against
`render_text` itself, not deferred to `protolens` (spec 0111, which doesn't
exist yet):

- **Nesting invariant**: a fixture with at least 3 levels of nesting (message
  containing message containing message, plus a sibling scalar at each level)
  — assert every child `NodeSpan`'s `text_range` *and* `raw_range` are each
  strictly contained within its parent's, via a single flat containment check
  against each `Vec<NodeSpan>` entry directly (both are absolute/flat coordinate
  spaces spanning the whole document — Design rationale, "Nesting" — no
  recursive re-slicing needed for either).
- **Line-number survival across the group-splice path**: a fixture with an
  open-ended or mismatched-end group (guaranteed to trigger
  `render_group_field`'s post-hoc splice, Design rationale) — assert every
  descendant's `text_range`, recorded *before* the splice runs, still points
  at the correct lines *after* it runs. This is the direct test of the
  line-based-vs-byte-based design decision (Design rationale).
- **Any expansion**: a fixture with a `google.protobuf.Any` field — assert the
  `value {}` wrapper node itself gets a `NodeSpan` with `type_fqdn` set to the
  *resolved* type, distinct from the container's own type and from its
  recursively-rendered children's spans.
- **MessageSet expansion**: analogous fixture for
  `message_set_field.rs`'s `Item {}`/`message {}` wrapper nodes, reusing spec
  0108's `message_set_proto2.proto` fixture.
- **Scalar leaf nodes**: assert `type_fqdn` is `None`.
- **`raw_range` fidelity**: for a nested node (not just a top-level one —
  `raw_range` is absolute for every node, not only top-level ones), extract
  its `raw_range` sub-slice directly from the original top-level `buf` and
  confirm it independently re-decodes correctly under that node's own
  (declared or resolved) type.

### §4 — `decode_and_render` initial-level / header-suppression parameters

```rust
pub fn decode_and_render(
    buf: &[u8],
    root_desc: Option<&MessageDescriptor>,
    annotations: bool,
    indent_size: usize,
    expand_any: bool,
    hide_unknown_fields: bool,
    expand_message_set: bool,
    initial_level: usize,   // NEW — was implicitly 0 (`LEVEL.with(|c| c.set(0))`)
    emit_header: bool,      // NEW — was implicitly `annotations` (see below)
) -> Vec<u8> {
    ...
    LEVEL.with(|c| c.set(initial_level));
    if annotations && emit_header {
        out.extend_from_slice(b"#@ prototext: protoc\n");
    }
    ...
}
```

`emit_header` is needed because today's unconditional `#@ prototext: protoc`
header (written whenever `annotations` is on) is only meaningful for a top-level
document; a sub-render destined to be spliced into an existing document's text
must not repeat it. This is an **additive, backward-compatible signature
change** (settled — not a parallel lower-level entry point): existing callers
pass `initial_level: 0, emit_header: annotations` to preserve current behavior
exactly, matching each parameter's pre-existing implicit default
(`LEVEL.with(|c| c.set(0))`, and `emit_header` mirroring today's unconditional
`annotations`-gated header write).

### §5 — `decoder` module retirement plan

Once `ProbeSink` covers `len_field.rs:51` and `decoder/packed.rs`'s mirrored
probe call:

- Remove `decoder::parse_message`, `decoder::ingest_pb`, `decoder/types.rs`
  (`ProtoTextMessage`/`ProtoTextField`/`ProtoTextContent`), `decoder/packed.rs`.
- Remove `serialize/common/format.rs::format_protoc_value` (dead, only consumer
  of the removed types).
- Replace `roundtrip.rs::len_wire_type_on_varint_field_sets_type_mismatch_flag`
  with an equivalent test against the `ProbeSink`-based replacement, preserving
  the same regression coverage (a LEN wire type on a declared-`int32` field must
  still be flagged as malformed/mismatched via whatever the replacement's
  equivalent signal is).
- Correct `docs/prototext/design.md`'s stale claim (lines ~379–382) that `render_as_bytes`
  depends on `ingest_pb`'s IR, while touching this area regardless.

---

## Implementation Plan

Staged, each step gated by its own test run *before* starting the next step's
code — not landed as one undifferentiated diff, given the number of files
touched (§ Files changed) and the module retirement involved (§5).

1. **`Sink` trait + `TextSink` impl** (Goal 1; Goal 2 excluding the
   Any/MessageSet refactor). Define the trait (§1); route `render_message`/
   `len_field.rs`/`packed.rs`/`scalar.rs`/`annotations.rs` through it for
   `TextSink` only.
   **Gate**: full existing `render_text` test suite passes with zero diff
   (byte-for-byte identical output).
2. **`any_field.rs`/`message_set_field.rs` → `Sink`-generic** (rest of Goal 2,
   Design rationale's Any/MessageSet subsection).
   **Gate**: same suite, zero diff — and explicitly confirm the suite's
   existing fixtures actually exercise the Any/MessageSet expansion paths;
   add coverage first if they turn out thin rather than assuming they're
   sufficient.
3. **`decode_and_render`'s new parameters** (Goal 4, §4), including updating
   every real call site (§ Files changed): `prototext-pyo3/src/lib.rs`'s two
   direct calls, and `prototext-core/src/lib.rs::render_as_text`'s one
   internal call (the actual `prototext` CLI's call path, via
   `prototext/src/run.rs`) — all three land together, since the crate won't
   compile otherwise.
   **Gate**: new unit tests for `initial_level != 0` (correct indentation
   depth) and `emit_header: false` (header suppression); existing suite still
   zero diff for default-parameter callers; `prototext-pyo3`'s own test suite
   (`prototext-pyo3/tests/test_codec.py`) still passes after its call sites
   are updated; `prototext`'s own CLI-level tests (`prototext/tests/`) still
   pass after `render_as_text`'s call site is updated.
4. ~~**`ProbeSink`** (Goal 3), wired into `len_field.rs:51` and
   `decoder/packed.rs`'s mirrored probe site (§2).~~ **Done.**
   **Gate**: spec-0097's existing cascade fixtures pass unchanged; new tests
   for Open Issue #1's group-malformity rollup — both that the rollup doesn't
   regress currently-passing cascades, and that cases previously accepted
   despite an internally malformed nested group now correctly fall through to
   string/bytes. — met: `probe_sink_recognizes_valid_nested_message` and
   `probe_sink_rolls_up_nested_group_malformity` (`render_text/mod.rs`
   `#[cfg(test)] mod tests`); full suite 101 → 103, zero regressions;
   `reuse lint` clean. Also added `Sink::treat_len_as_opaque`/`tracks_level`
   (§1) beyond the trait's originally-settled 6 methods — see Open Issue #2.
5. **`IndexingTextSink`/`NodeSpan`** (Goal 3, §3).
   **Gate**: the §3 "Tests" list — nesting invariant, line-number survival
   across the group-splice path, Any/MessageSet wrapper-node spans, scalar
   `type_fqdn: None`, `raw_range` fidelity.
6. ~~**`decoder` module retirement** (Goal 5, §5).~~ **Done.**
   **Gate**: zero remaining references to
   `decoder::parse_message`/`ingest_pb`/`ProtoTextMessage`/etc. anywhere in
   the crate; the replaced `roundtrip.rs` test passes; `reuse lint` clean. —
   met: removed `prototext-core/src/decoder/` (`mod.rs`, `codec.rs`,
   `types.rs`, `packed.rs`) and the now-fully-dead
   `serialize/common/format.rs` (its only consumer was `decoder`'s IR
   types); removed `pub mod decoder;` from `lib.rs` and the dead
   `probe_message` wrapper (only caller was `decoder/packed.rs`, since
   `len_field.rs` already calls `ProbeSink` directly);
   `len_wire_type_on_varint_field_sets_type_mismatch_flag` replaced with an
   equivalent `render_as_text`-based assertion on the `TYPE_MISMATCH`
   annotation; corrected `docs/prototext/design.md` (removed the obsolete
   Intermediate-representation/Decode-path sections, the stale
   `render_as_bytes`/`ingest_pb` claim, and the `decoder/` crate-layout
   entry). Full workspace suite zero-diff; clippy shows only the 4
   pre-existing (Step 8) `too_many_arguments` warnings; `reuse lint` clean.
7. ~~**Zero-cost benchmark checkpoint** (Open Issue #3) — run once steps 1–6 are
   otherwise complete; does not gate any individual step, but must pass before
   this spec's `Status` moves to `implemented`.~~ **Done.**
   **Gate**: no wall-clock or structural regression evidence. — met: no
   Criterion bench harness existed anywhere in this repo (the one that
   produced `docs/prototext/performance.md`'s numbers lived only in a sibling private
   repo); ported it to `prototext-core/benches/codec.rs` with a new committed
   fixture (`prototext-core/fixtures/descriptor.pb` +
   `descriptor_protoc.txt`). Compared a `b517191` (pre-refactor) worktree
   against `main` (post-refactor) for a like-for-like, within-repo A2
   measurement — the pre-refactor value (415–417 µs) fell inside the
   post-refactor binary's own run-to-run noise band (287–449 µs across 3
   runs; this sandbox is a noisy single virtualised core, see
   `docs/prototext/performance.md`'s environment note). Structurally, `grep -rn "dyn
   Sink"` across `prototext-core/src` returns no matches — `Sink` is
   consumed exclusively via `<S: Sink>` static generics, monomorphized and
   inlined the same as the pre-refactor direct calls, so there is no vtable
   indirection for the refactor to have introduced. Full writeup:
   `docs/prototext/performance.md` §"Spec 0110 — Sink-based render refactor (zero-cost
   checkpoint)". `ProbeSink`-vs-`decoder::parse_message` comparison from
   Open Issue #3's original plan is moot — `decoder::parse_message` no
   longer exists (deleted in Step 6).
8. ~~**`clippy::too_many_arguments` cleanup** — deferred until after steps 5/6
   land (so the bundling struct shape only needs designing once, accounting
   for whatever `IndexingTextSink` needs too), then done in one pass across
   all flagged functions together: `render_group_field`, `render_len_field`,
   `render_any_expansion`, `render_message_set_expansion` (bundle
   `field_number`/`field_schema`/`tag`/`raw_range` into a context struct,
   mirroring the existing `ScalarCtx` pattern), and `decode_and_render`
   (accept an options struct instead of loose scalar parameters — mirroring
   `RenderOpts`, `lib.rs`). Must pass before this spec's `Status` moves to
   `implemented`.~~ **Done.**
   **Gate**: `cargo clippy --release --all-targets` shows zero warnings; full
   workspace suite zero-diff; `reuse lint` clean. — met: added
   `FieldCtx<'a>` (`field_number`/`field_schema`/`tag`, `len_field.rs`) used
   by `render_len_field`, `render_group_field`, `render_any_expansion`, and
   `render_message_set_expansion` — `raw_range`/`raw_start` deliberately
   stayed a separate per-call parameter rather than joining the struct,
   since `render_group_field` only has `raw_start: usize` at entry (its end
   isn't known until parsing completes), making a uniform `Range<usize>`
   field awkward across all 4 call sites. Added `DecodeRenderOpts`
   (`annotations`/`indent_size`/`expand_any`/`hide_unknown_fields`/
   `expand_message_set`/`initial_level`/`emit_header`, with a `Default` impl
   mirroring `RenderOpts`) used by `decode_and_render` and
   `decode_and_render_indexed`, reducing both from 9 params to 3. All call
   sites updated (`lib.rs`, `prototext-pyo3/src/lib.rs`,
   `prototext/tests/node_span.rs`, `render_text/mod.rs`'s own tests).

---

## Open Issues and Challenges

1. ~~**`ProbeSink` group-malformity rollup is a behavior change**, not a pure
   refactor (§2) — needs explicit test coverage confirming spec 0097's existing
   cascade fixtures aren't affected by the stricter (rolled-up) malformity count,
   and confirming cases that previously rendered as a message (accepted despite
   an internally malformed group) now correctly fall through to string/bytes.~~
   **Resolved**: the rollup is a *structural* guarantee, not something a `Sink`
   implementation must remember to do — because every recursion level is
   threaded the same `&mut S` (§1, "`render_message<S: Sink>`" prose), a
   nested group's malformities land in the caller's total automatically, with
   no return-value hand-off step to get wrong. This forecloses the exact bug
   `decoder::parse_message`'s `WT_START_GROUP` arm has today. Test coverage
   confirming the resulting counts match expectations:
   `probe_sink_rolls_up_nested_group_malformity` (`render_text/mod.rs`).

   **Amended 2026-08-06.** The rollup covered malformities found *inside*
   a group but not the group itself failing to close: `ProbeSink::
   end_nested` was an empty body, so a missing `END_GROUP` cost nothing.
   That is the one defect the probe's other two tests cannot catch, since
   an unterminated group consumes the rest of the buffer and therefore
   satisfies `next_pos == data.len()` as well. The cost was real — a
   plain string whose last byte happens to be a `START_GROUP` tag
   rendered as a nested message. `google.api.method_signature`'s value
   `"ad_break,update_mask"` is such a string: it parses as fixed64
   `d_break,`, fixed32 `pdat`, fixed32 `_mas`, and a trailing `k` = 0x6b,
   which is field 13, `START_GROUP`. `end_nested` now counts
   `close_facts.is_none()`, which under `ProbeSink` is unambiguous —
   `treat_len_as_opaque` returns before any nested *message* is opened,
   so the only nesting that reaches it is a group. A *mismatched*
   `END_GROUP` is deliberately still not counted: it closes, and the
   render annotates it. Test:
   `probe_sink_rejects_a_group_that_never_closes` (`render_text/mod.rs`).
2. ~~**Exact `Sink` trait shape — event granularity only.**~~ **Resolved, then
   grew by two during implementation**: §1 originally settled the trait at 6
   methods — `scalar_field`, `begin_nested`, `end_nested`, `malformed`, plus
   `virtual_scalar`/`begin_virtual_nested` for the Any/MessageSet wrapper-node
   calls (Design rationale's Any/MessageSet subsection). Implementing `ProbeSink`
   (step 4) surfaced two more, both default-`false`/`true` hooks needed to keep
   `ProbeSink` a genuinely passive, read-only probe: `treat_len_as_opaque`
   (resolves the §2 shallow-LEN-handling ambiguity — confirmed with the spec's
   author) and `tracks_level` (stops `ProbeSink`'s mandatory group recursion
   from mutating the shared thread-local `LEVEL` counter that the in-progress
   outer `TextSink` render, which invoked the probe, still depends on for its
   own indentation). Richer per-method payload types (`TagFacts`, `ScalarValue`,
   `NestedKind`, `MalformedKind`) keep this event cut manageable without a
   combinatorial explosion of methods — see §1 for the full shape.
3. ~~**Zero-cost verification**: this spec's premise (monomorphization + inlining
   makes the `Sink` abstraction's genericity free) needs confirming, not just
   assuming from the generic Rust idiom — with priority weighted by which
   `Sink` sits on a hot path. `TextSink` is the actual hot path (`prototext
   decode`'s production output) and is the primary concern: the abstraction
   must not itself slow it down. `IndexingTextSink` is not on a hot path (used
   interactively, low volume, by `protolens`). `ProbeSink` may run on a hot
   path (schema-less messages), a secondary but real concern. Resolution: a
   post-implementation checkpoint, not a pre-implementation blocker — re-run
   the existing `A2 decode_and_render` Criterion bench (`docs/prototext/bench-process.md`;
   current baseline in `docs/prototext/performance.md`: 205 µs · 80.4 MiB/s) post-refactor
   and confirm `TextSink` regresses ~0%; add a new bench for `ProbeSink` at the
   `len_field.rs:51` call site and compare against today's `decoder::parse_message`
   cost there; fall back to an `objdump` spot-check (per `docs/prototext/bench-process.md`)
   for stray `call` instructions into `TextSink`-only code (e.g. the
   splice-based backtracking path) if either number regresses unexpectedly.~~
   **Resolved** (Step 7): no wall-clock regression evidence (pre-refactor A2
   falls inside the post-refactor binary's own measurement noise) and no
   structural regression evidence (`Sink` has zero `dyn` call sites — see
   Step 7's gate note and `docs/prototext/performance.md`). The `ProbeSink`-vs-
   `decoder::parse_message` comparison in the original plan is moot:
   `decoder::parse_message` was deleted in Step 6, so there is no longer a
   pre-refactor baseline to compare `ProbeSink` against.
4. ~~**`decode_and_render`'s new parameters**~~ — **Resolved**: additive
   signature change (§4), not a parallel entry point. Existing callers
   (`prototext` CLI, `prototext-pyo3`) pass `initial_level: 0, emit_header:
   annotations` to preserve current behavior exactly.
5. **`NodeSpan` shape for downstream consumers**: whether emission-order
   `Vec<NodeSpan>` (relying on containment for structure) is sufficient, or
   whether an explicit `parent_index` field earns its keep now rather than being
   added later — this spec has no consumer to validate the answer against;
   revisit once spec 0111 is underway.
6. **`NodeSpan.type_fqdn` representation**: an owned `String` per node (simple,
   but duplicates the same FQDN across every occurrence of a repeated field) vs.
   an interned/shared handle into the schema pool already held elsewhere in
   `prototext-core` — not decided; affects `IndexingTextSink`'s per-node memory
   cost on large documents with many repeated Any/MessageSet occurrences.

---

## Files changed (anticipated)

| File | Change |
|---|---|
| `prototext-core/src/serialize/render_text/sink.rs` (new) | `Sink` trait; `TextSink`, `ProbeSink`, `IndexingTextSink` implementations; `NodeSpan`/`GroupCloseFacts` types |
| `prototext-core/src/serialize/render_text/mod.rs` | Generalize `render_message`/`decode_and_render` over `S: Sink`; add `initial_level`/`emit_header` parameters; add new `decode_and_render_indexed` entry point (§3) |
| `prototext-core/src/serialize/render_text/helpers/len_field.rs` | Route writes through `Sink`; replace `decoder::parse_message` probe call with a `ProbeSink`-based walk |
| `prototext-core/src/serialize/render_text/helpers/any_field.rs` | `render_any_expansion` becomes `Sink`-generic — wrapper lines (`type_url: "..."`, `value {`) routed through `scalar_field`/`begin_nested`/`end_nested` instead of hand-written writes |
| `prototext-core/src/serialize/render_text/helpers/message_set_field.rs` | `render_message_set_expansion` becomes `Sink`-generic — wrapper lines (`Item {`, `type_id: N`, `message {`) routed through `Sink` calls |
| `prototext-core/src/serialize/render_text/packed.rs` | Route through `Sink` |
| `prototext-core/src/serialize/render_text/helpers/scalar.rs`, `annotations.rs` | Route through `Sink` |
| `prototext-core/src/decoder/mod.rs`, `decoder/types.rs`, `decoder/packed.rs` | Removed |
| `prototext-core/src/serialize/common/format.rs` | Remove dead `format_protoc_value` |
| `prototext/tests/roundtrip.rs` | Replace `ingest_pb`-based test with `ProbeSink`-based equivalent |
| `prototext-pyo3/src/lib.rs` | Update both direct `decode_and_render` call sites (`lib.rs:139`, `lib.rs:251`) to pass the two new trailing args (`initial_level: 0`, `emit_header` matching current `annotations` semantics) — mechanical, no Python-facing API change |
| `prototext-core/src/lib.rs` | Update `render_as_text`'s internal `decode_and_render` call site (`lib.rs:107`) to pass `initial_level: 0, emit_header: opts.include_annotations` — mechanical, no change to `render_as_text`'s own public signature or `RenderOpts` |
| `docs/prototext/design.md` | Correct stale claim re: `ingest_pb`/`render_as_bytes` |
