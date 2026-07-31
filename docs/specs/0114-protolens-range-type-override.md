<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0114 — `protolens`: single-range type override

Status: implemented
Implemented in: 2026-07-13
Refs: `docs/specs/0109-protolens-interactive-schema-inference.md`,
`docs/specs/0110-render-sink-unification.md`,
`docs/specs/0111-protolens-v1-decode-navigate-extract.md`,
`docs/specs/0113-protolens-tui-refinements.md`
App: protolens

---

## Background

Spec 0109 laid out `protolens`'s full override design at a high level (Goals
3–4): per-node type override in two scopes (exact position vs.
field-number pool), candidate ranking via `score_all`, unlimited undo,
save-project/export-`.desc`. Spec 0111 shipped the first slice (decode/
navigate/extract) and explicitly deferred all of this to future work, while
sketching a placeholder picker UI in its Annex C "Phase 3" (a centered
modal overlay).

This spec is the next slice: **override the type of a single byte range**,
end to end — pick a candidate type, re-render the affected range under it.
It deliberately narrows spec 0109's scope further than even 0111's sketch
anticipated:

- **Exact-position scope only** — spec 0109's "field-number pool" scope
  (retype every occurrence of a field at once) is deferred.
- **One active override at a time** — applying a new override replaces
  whatever was previously active; there is no persistent *set* of
  overrides yet. Managing a set (multiple simultaneous overrides,
  activate/deactivate/delete) is explicitly deferred to a follow-up spec.
- **No undo/redo, no save/export** — spec 0109 Goals 5–7 remain untouched.

This spec **supersedes 0111 Annex C's "Phase 3" sketch**: the picker is an
ephemeral **right-hand split pane**, not a centered modal. Rationale: a
side pane keeps the main tree pane visible and navigable at the same time
(via a focus toggle — see §2), which is a prerequisite for a later
live-preview feature (re-rendering the main pane in real time as the user
moves through the candidate list) even though that live-preview behavior
itself is not implemented by this spec.

**Naming note**: "override" is the *architectural* term throughout this
spec, matching spec 0109's own vocabulary (Goal 3, "Type override of a
node") — the internal module (§ "Files changed") keeps that name. The
concrete UI surface (the `:type-as` command, the `t` key) uses a
friendlier, more readable verb; this is a deliberate UI/architecture
naming split, not an inconsistency.

---

## Goals

1. **Override target**: pressing `t` with the cursor on a message- or
   group-shaped node opens the override pane, scoped to that node's byte
   range (tag/length-prefix excluded — see §1). On a scalar/leaf node, `t`
   is a no-op with a status-line message. The tree gains a virtual
   encompassing wrapper node so that "the node under the cursor" is always
   unambiguous, including on the first visible line (§1.1).
2. **Override pane**: an ephemeral right-hand split (§2), listing a pinned
   `<raw / no type>` entry (§3.1) followed by candidate types for the
   target range, sorted lexicographically or by inferred plausibility
   (default — reusing `score_all`, no new inference logic), `a` toggling
   between the two. On open,
   the highlighted row defaults to the first ranked candidate, not the
   pinned raw entry (§3.1). Vim-style in-pane search (`/`, `?`, `n`).
   `Enter` applies the highlighted candidate (or raw) and closes the pane;
   `Esc` cancels and closes it; `t` also toggles the pane closed from
   either pane's focus.
3. **Focus toggle**: `Tab` moves focus between the main pane and the
   override pane while the latter is open; the main pane stays fully
   navigable with the override pane still visible.
4. **`:type-as <FQDN>` / `:type-as-raw`**: apply a type, or raw/no-type,
   directly, bypassing the pane entirely — two full command names, not a
   flag (see §7 for why exact-match-wins prefix semantics make this the
   cleaner shape). Same validation and application logic as picking a
   candidate (or the pinned raw entry) from the pane.
5. **Unambiguous `:`-command prefix matching**, vim-style, exact match
   always winning over prefix ambiguity (e.g. `:ty` resolves to `:type-as`
   if unambiguous; typing `:type-as` in full always resolves to itself even
   though it's also a prefix of `:type-as-raw`) — a small shared change to
   the command dispatcher, benefiting `:extract` as well as the new
   `:type-as`/`:type-as-raw`.
6. **Re-render on apply**: once an override is applied, the target range is
   re-decoded under the newly-selected type and spliced into the main
   pane's rendering in place of its old subtree (§5).
7. **Candidate-list caching** (§6), so that reopening the pane for a
   previously-seen range, or scrolling within the currently-open pane's
   list, does not require redundant `score_all` calls.
8. **Tab-completion with cycling** (§7, spec 0113 D26): command-name
   completion (shared infra, also benefiting `:extract`) and, for
   `:type-as`, FQDN-argument completion — both using the same
   longest-common-prefix-then-cycle model, `Tab`/`Shift-Tab`.

## Non-goals

- **Field-number-pool override scope** (spec 0109 Goal 3's second scope) —
  exact-position only in this slice.
- **A set of overrides** (multiple simultaneous, independently
  activate/deactivate/delete-able overrides) — spec 0109's fuller model;
  this slice supports exactly one active override, replaced wholesale each
  time a new one is applied. Explicitly deferred to a follow-up spec, per
  the discussion that produced this one.
- **Undo/redo** of override decisions (spec 0109 Goal 5).
- **Save-project / export-`.desc`** (spec 0109 Goals 6–7).
- **Live preview**: re-rendering the main pane in real time as the cursor
  moves within the override pane's candidate list, before a selection is
  applied. This is the motivating use case for the right-pane-plus-focus-
  toggle architecture (§2), but is not implemented here — only the
  Tab-focus mechanics are, so a later spec can add live preview without
  re-architecting the layout.
- **Cache persistence** across sessions — the caches in §6 are in-memory
  and session-scoped only, matching the jumplist's own precedent (Annex B,
  spec 0111).
- **File-path Tab-completion for `:extract`'s `<path>` argument** — Goal 8/
  §7's Tab-completion only covers command names and `:type-as`'s FQDN
  argument (spec 0113 D26); `:extract`'s path stays plain-typed, matching
  its existing no-file-browser precedent (spec 0113 D21).

---

## Specification

### §1 — Override target and tag exclusion

The override applies to a *message's* interpretation of a byte range, not a
field's — so the wire tag (and, for length-delimited fields, the length
prefix) must be excluded from the range that gets re-scored/re-decoded.
This is already solved: `extract.rs::extract_binary(blob, &node.span.raw_range,
is_message)` strips exactly this (tag+length for messages, leading tag for
groups), with existing regression tests (spec 0111/0113). The override
range is defined as this already-stripped byte slice — reuse the function,
don't reimplement the stripping.

`t` (and `:type-as`) are only valid when the cursor's node is message- or
group-shaped (`is_message` would be `true` for it); otherwise a status-line
message ("cannot override: not a message/group field") and no pane opens.

#### §1.1 — Amendment: the root/field-1 ambiguity and the virtual wrapper

**Discovered:** interactive testing of Task #17 (this spec's §1/§2
skeleton). **Status:** implemented (2026-07-13).

**Problem.** With the cursor on the tree's first visible line, it is
ambiguous whether "the node under the cursor" denotes the *whole given
protobuf* or its *first top-level field* — today's top-level nodes are not
children of anything (`decode.rs::build_tree`'s stack-based ingestion
never gives them a parent; see also spec 0113 D16, which already
special-cases root-level fold behavior for the same reason). This
ambiguity pre-dates 0114 but was cosmetic until now: `t`'s target-range
determination (§1) needs an unambiguous node/range for every cursor
position, including the first line, and there currently isn't one.

**Design — a genuinely valid wrapper protobuf.** Rather than special-case
the root, or invent a zero-width sentinel node, construct real,
well-formed wire bytes: prepend a normal tag+length-varint prefix for a
field 1 (`WT_LEN`) to the actual input blob, and decode the augmented blob
under a synthetic one-message, one-field descriptor whose field 1 is
message-typed, `type_name` = the already-resolved root type's FQDN
(`determine_root_type`, `decode.rs`). This is decoded through the
existing, **unmodified** `decode_and_render_indexed`/`Sink`/`NodeSpan`
machinery: the wrapper's own `NodeSpan` is produced by the same
`begin_nested`/`end_nested` path as any other message node, so no change
is needed to `prototext-core`.

Concretely, in `decode.rs`:

1. Build one synthetic `FileDescriptorProto` with a single `DescriptorProto`
   (e.g. `Wrapper`) containing one field: number 1, `TYPE_MESSAGE`,
   `type_name = ".<resolved_root_fqdn>"`.
2. Register it with `DescriptorPool::add_file_descriptor_proto` — verified
   present and fit for purpose in `prost-reflect` 0.16.3 (the version
   pinned in `Cargo.lock`): "types referenced by the file must be defined
   either in the file itself, or in a file previously added to the pool,"
   which the resolved root type already is, since it came from `ctx.pool()`
   itself. `decode()`'s `ctx` parameter becomes `&mut DescriptorContext` to
   accommodate this (`DescriptorContext::pool` was previously read-only
   after `load`). The synthetic file's own `dependency` list must name the
   resolved root type's parent file (`root_desc.parent_file().name()`) —
   `add_file_descriptor_proto` resolves cross-file `type_name` references
   the same way `protoc` does, via explicit imports, not implicit pool-wide
   visibility; omitting this makes registration fail with "name '...' is
   not defined" even though the type is already in the pool. The wrapper
   message/file names are namespaced by the resolved root type's own FQDN
   (`protolens_internal.Wrapper_<root_fqdn with '.' replaced by '_'>`) so
   that decoding the same `ctx` twice with two different root types (e.g.
   an `:extract` round trip re-opening a submessage as its own root) never
   collides; a second `decode()` call for an already-registered wrapper
   reuses it via `DescriptorPool::get_message_by_name` rather than
   re-registering (which would otherwise be a duplicate-file skip per
   `add_file_descriptor_proto`'s own documented behavior).
3. Prepend the real tag+length-varint prefix to `blob` before calling
   `decode_and_render_indexed`, passing the `Wrapper` descriptor as
   `root_desc` instead of the actual root type's descriptor.

This adds exactly one new top-level node — the wrapper's field 1 — above
what is today the top level; the given protobuf becomes that field's
payload, and every existing top-level node becomes its child. As a side
effect, this also shrinks spec 0113 D16's root-level special case: today
it applies to potentially many top-level siblings with no parent; under
the wrapper, only the wrapper itself (a singleton) has no parent.

**Display corrections** (display-time only — no change to how ranges/paths
are computed or stored):

- **Half-open ranges.** Status-line/pane range display switches from
  today's inclusive `[start..end]` (`bytes[{start}..{end-1}]`,
  `saturating_sub(1)`) to half-open `[start..end)` — cleanly represents an
  empty payload (`[5..5)`) and removes the `saturating_sub` hack.
- **Payload-only display for message/group nodes.** A message/group node's
  *displayed* start coordinate strips its own tag(+length-prefix) width —
  i.e. it displays the range of its payload, not the field including its
  wire tag. Scalars are unaffected (their tag-inclusive display is
  unchanged). This is what makes the wrapper's own field-1 node display as
  `[0..n)` rather than `[-p..n)`: its payload *is* the given protobuf.
- **Global offset subtraction.** Let `p` = the wrapper's own tag+length
  width in bytes. Every displayed `raw_range` (both coordinates, for every
  node) is shown as `raw_range - p`. Since prepending the wrapper prefix
  shifts every real node's underlying `raw_range` by exactly `+p`
  uniformly (a pure byte-offset translation, independent of nesting
  depth), this exactly restores today's pre-wrapper numbers for every
  pre-existing real node. Combined with payload-only stripping, the
  wrapper's own node displays as `(0 + p) - p, (p + n) - p = [0, n)` —
  matching what is today displayed for the whole blob.
- **Path: drop the leading leg.** Every real node's *internal* positional
  path (spec 0113 D25) gains a genuine leading `/1` segment (descent into
  the wrapper's sole field). The *displayed* path strips that leading
  segment, restoring today's exact path strings for every real node
  (internal `/1/3` → displayed `/3`, unchanged). The wrapper's own node has
  internal path `/1`; with the leading leg dropped, it displays as bare
  `/` — the same convention D25 already reserves for "the root."

Net effect: the main pane gains one new node — the wrapper's sole
field, standing in for the entire given protobuf — rendered like any
other message node (an opening line and a matching closing line, one
level of indentation added to everything else). Its opening line
displays `[0..n) /`, and every other line's displayed range/path is
byte-for-byte unchanged from today. `t` on that opening line is now
well-defined and unambiguous (it is message-shaped, so it's a valid
override target — same as any other message node, no special-casing in
`toggle_override` itself).

**Out of scope for this amendment:** this resolves the root/field-1
ambiguity only. It does *not* fix the separate `t`-detection bug where
heuristically-probed, schema-unresolved message nodes anywhere in the
tree are indistinguishable from scalars — see §1.2.

#### §1.2 — Amendment: `NodeSpan` shape discriminator (`is_message`)

**Discovered:** interactive testing of Task #17 — `t` refused on every
node when decoding under a schema that declares no fields for the target
(e.g. `--type google.protobuf.Empty`). **Status:** implemented
(2026-07-13).

**Problem.** `toggle_override` (and, previously, `extract.rs`) uses
`NodeSpan::type_fqdn.is_none()` as its scalar/message discriminator. But
`type_fqdn` is `None` in two unrelated situations:

1. **True scalars** — `IndexingTextSink::scalar_field` (`sink.rs`) always
   pushes `type_fqdn: None`, unconditionally.
2. **Message/group-shaped fields with no resolved schema** —
   `begin_nested`/`end_nested` push `type_fqdn: declared_type_fqdn(field_schema)`,
   which is `None` whenever `field_schema` is `None`. This includes the
   unknown-LEN-field cascade (`helpers/len_field.rs`, spec 0097): a
   schema-less field is probed (`ProbeSink`), and if it parses cleanly as
   a nested message, it's rendered via `begin_nested(..., field_schema:
   None, kind: NestedKind::Message, ...)` — structurally a message, but
   its `NodeSpan` is indistinguishable from a scalar's.

Decoding under a schema that declares no fields for the target type (or
any field genuinely absent from the schema) puts every nested field into
case 2 — `type_fqdn.is_none()` is then true everywhere, and `t` is
refused on every node.

**Heuristics considered and rejected** (both are line-format-derived
proxies for something that should just be a field on `NodeSpan`):

- Sniffing the rendered opening line for a trailing `" {"` — fragile,
  coupled to `TextSink::begin_nested`'s exact formatting, not a
  structural signal.
- `text_range` spanning ≥2 lines ⟹ message — invalidated by
  `render_packed` (`packed.rs`), which writes one line per element within
  a single `scalar_field` call; a packed-repeated scalar can span many
  lines too.

**Design.** Add `pub is_message: bool` to `NodeSpan` in `prototext-core`,
set unconditionally and independently of `type_fqdn`, at the two push
sites in `IndexingTextSink`: `scalar_field` → `false`;
`begin_nested`/`end_nested` → `true`. `type_fqdn`'s existing semantics are
unchanged (still `None` for scalars *and* unresolved messages — "declared/
resolved type name, if known"); `is_message` is a purely orthogonal shape
discriminator.

A 3-way `Scalar`/`Message`/`Group` discriminator is not needed: `extract.rs
::message_payload_range` already self-detects group-vs-message from the
wire tag byte itself (`WT_LEN` vs `WT_START_GROUP`), not from `NodeSpan`
— a single bool is exactly what both `toggle_override` and
`extract_binary`'s call sites need.

**Blast radius**: `NodeSpan { .. }` struct literals appear only in
`sink.rs` (real construction) and in `tui.rs`/`extract.rs` test fixtures
— no other crate constructs one by literal.

**Call-site updates**:
- `tui.rs::toggle_override`: `type_fqdn.is_none()` → `!span.is_message`.
- `extract.rs` (both call sites): `let is_message =
  node.span.type_fqdn.is_some();` → `node.span.is_message` directly — same
  behavior for cases already covered by its regression tests, correct now
  for the previously-mishandled unresolved-message case too.

#### §1.3 — Amendment: widen eligibility to length-delimited scalars

**Discovered:** interactive testing (2026-07-14) — `t`/`:type-as`
unconditionally refused every field currently rendered as a scalar
`string`/`bytes` (e.g. `tracking_tag: "\002tpc_change_id\003..." #@ string
= 2`), even when such a field could plausibly carry an embedded message
the active schema doesn't declare. **Status:** implemented (2026-07-14).

**Design decision** (confirmed with the user): an override that produces
an invalid/nonsensical reinterpretation is acceptable — `protolens` should
allow the attempt and let the user judge the result, not pre-emptively
block it. This is the same trust model the Any/MessageSet auto-override
machinery (spec 0120) already relies on unconditionally, and
`splice_override` already reports a bad reinterpretation gracefully via
its `Result<(), String>` return rather than panicking.

**Design.** New shared eligibility predicate,
`fn can_override(&self, idx: usize) -> bool`: `span.is_message ||
span.wire_type == WT_LEN` (§1.2's `is_message` field, plus spec 0115's
`wire_type` discriminator). A packed-repeated element is *not*
inadvertently included by the `WT_LEN` half: packed encoding only ever
applies to primitive numeric/bool/enum kinds, so a packed element's own
`wire_type` (`sink.rs`'s `elem_wire_type`) is always
`WT_VARINT`/`WT_I32`/`WT_I64`, never `WT_LEN` — the packed field *as a
whole* remains excluded exactly as before, with no new container-level
`NodeSpan` needed. `toggle_override` and the shared `type_as` logic
(`:type-as`/`:type-as-raw`) both call `can_override` in place of the bare
`is_message` check, with the refusal message updated to "cannot override:
not a message/group or length-delimited field."

### §2 — Pane layout and focus model

Ephemeral horizontal split, structurally the same `ratatui::Layout` split
as 0111 Annex C's Phase 5 mockup, but transient (opened by `t`, not a
persistent toggle):

```
┌──────────────────────────────────┬───────────────────────────────┐
│ Main tree pane (Percentage(65))    │ Override — range [212..8841]     │
│▾ message_type {                    │   <raw / no type>                │
│▸   name: "DescriptorProto"         │ > google.protobuf.DescriptorProto│
│    field { ... }                   │   google.protobuf.FieldDescrip...│
│                                     │   google.protobuf.EnumDescrip... │
├─────────────────────────────────────┴───────────────────────────────┤
│ status line                                                           │
├───────────────────────────────────────────────────────────────────-──┤
│ command/message line                                                  │
└───────────────────────────────────────────────────────────────────-──┘
```

- `t` on a valid target opens the pane and moves focus to it; the
  highlighted row defaults to the first ranked candidate (§3.1).
- `Tab` toggles focus between main pane and override pane while the
  override pane is open; the main pane's cursor/navigation keys behave
  exactly as in single-pane mode when it has focus.
- `t` closes the pane (cancelling, same as `Esc`) regardless of which pane
  currently has focus — so the user is never stuck unable to reach a close
  key after using `Tab` to inspect the main pane.
- Below a minimum terminal width (matching Annex C's Phase-5 precedent,
  e.g. 100 columns), `t` is refused with a status-line message rather than
  rendering an unusably narrow split.

### §3 — Candidate list and sort modes

#### §3.1 — The pinned `<raw / no type>` entry

Always the list's first row, regardless of sort mode, unaffected by `a`
toggling and excluded from in-pane search (§4) matching (it isn't an FQDN).
Selecting it assigns "no type" to the range: the range is explicitly
overridden to render as unschema'd/raw wire data — `prototext-core`
already supports this natively (`decode_and_render_indexed(slice, None,
opts)`, `root_desc: None`, the same "no schema active" mode
`DecodeRenderOpts`'s doc comment calls out — see §5).This is distinct from
*not having an override at all*: it's a recorded decision that this range
is deliberately opaque/unresolved, not merely unexamined yet.

**Not the default on open**: despite being the first row, the pane's
initial highlight sits on the first *ranked* candidate (the top of
whichever sort mode is active — §3.2), not on this pinned row. The
presumptive action on opening the pane is "accept the most likely
inferred type," not "mark this raw" — raw remains one `k`/`Up` press away.

#### §3.2 — Sort modes for the ranked candidates

Two sort modes, `a` toggling between them while the override pane has
focus — apply only to the ranked candidates below the pinned raw entry.

Later amended (spec 0137 implementation feedback): the active sort mode
no longer persists across successive `t` invocations for the session —
`t` resets it to the `Inferred` default every time the pane opens.

Later amended (post-0137 feedback): the sort-toggle key changed from
`a` to `i` (for "inferred"), avoiding ambiguity with the main pane's
own `a` (annotation-display toggle, spec 0133) and the management
pane's own `a` (entry active toggle, spec 0119).



- **Lexicographic**: all message/group types known to the loaded
  descriptor set, alphabetically by FQDN. Cheap — no `score_all` call (§6).
- **Inferred** (**default**): ranked by `score_all(range_bytes, graph,
  opts)` against the target range, descending score (ties broken by
  FQDN). This is the same scoring engine `protolens`'s own root-type
  determination already uses (spec 0111 Goal 2), applied here per-range
  instead of corpus-wide.

`j`/`k` (or arrows) move the highlighted candidate within the pane
(including onto the pinned raw entry); scrolling follows the same
windowed-rendering discipline as the main pane (Annex C).

### §4 — In-pane search

Vim-style, scoped to the override pane's candidate list (by FQDN text):
`/` search forward, `?` search backward, `n` repeat the last search in the
same direction. Confirming `/`/`?` with an empty typed pattern re-uses the
last active pattern, searching in the newly chosen direction (which may
differ from the direction that pattern was originally searched in — unlike
`n`, which always repeats in the same direction as last time).

Later amended (live interactive feedback) to also cover the main pane:
`/`/`?`/`n` work identically there, requiring main-pane focus (reusing the
command-line row as the search prompt — see `CommandLineKind` in
`tui.rs`), matching document-order nodes by their own opening line's
*current* rendered text — so a range whose type has been overridden (§5)
is matched post-override, not against the original rendering, with no
special-casing needed since overrides mutate the rendered text in place.
A folded-away match is still found (the whole document is searched, not
just currently-visible nodes) and its ancestors are then unfolded to
reveal it. This reassigned `?` away from the main pane's help-toggle
meaning (spec 0113 D22), which moved to `F1` instead — `Esc` and `F1` both
close the help overlay.

### §5 — Applying an override

`Enter` (pane focused, the raw entry or a candidate highlighted) or
`:type-as <FQDN>` / `:type-as-raw` (bypassing the pane):

1. Validate: the cursor is on a message/group node (§1); for `:type-as
   <FQDN>`, the FQDN also names a known message/group type in the loaded
   descriptor set. Either command reports a status-line error for any
   failure (unknown FQDN, FQDN names a scalar/enum type, cursor isn't on a
   message/group node) — same refusal `t` gives, just surfaced without ever
   opening a pane. In particular, neither command ever opens the override
   pane, so both work unconditionally, even in a terminal too narrow to
   display the pane (§2's minimum-width refusal only applies to `t`).
2. Replace any previously active override — this slice manages exactly one
   (Non-goals). Internal representation: `Option<MessageDescriptor>` (or an
   equivalent resolved-descriptor handle), **not** an empty-string FQDN
   sentinel — this matches `decode_and_render_indexed`'s own
   `root_desc: Option<&MessageDescriptor>` parameter exactly (§ below), so
   `None` flows straight through to the re-render call with no string
   round-trip, and there's no ambiguity between "no type" and "an
   as-yet-incomplete FQDN string."
3. Re-decode the target range's byte slice (§1) under the newly-selected
   type: call `prototext_core::decode_and_render_indexed(slice, root_desc,
   opts)` with `root_desc = Some(&new_desc)` for a type selection or `None`
   for raw (§3.1) — `decode_and_render_indexed` already supports `None` as
   "no schema active," so raw needs no special-casing beyond passing it
   through. `opts.initial_level` is set to the overridden node's original
   indentation level and `opts.emit_header = false` — both `DecodeRenderOpts`
   fields spec 0110 added specifically for this splice case (its own doc
   comment: "used for sub-renders destined to be spliced into an existing
   document's text, which must not repeat the header" — see spec 0110's
   Background and spec 0111 Non-goals, which named this exact deferred
   capability).
4. Splice the resulting lines and `NodeSpan`s into the main pane's existing
   `lines`/`tree`, replacing the old subtree's entries in place. The
   overridden node's `raw_range` (tag/length-inclusive) is unchanged; only
   its interior is now rendered under the new type.
5. Close the pane, return focus to the main pane, cursor remains on the
   (now re-rendered) overridden node.

### §6 — Candidate-list caching

`score_all` always scores every root entry registered in the compiled
graph in one call (`prototext-graph/src/score/walk.rs::score_all`) — there
is no cheaper "top-n only" mode, so caching should hold a generous, fixed
number of entries per computation regardless of how many the pane
currently needs to display; a taller pane (or a terminal resize) is then
still a cache hit as long as it's within that fixed cap, rather than
tracking "how many were needed last time."

Two tiers, both **inferred-order only** (lexicographic order needs no
per-range caching — see below), both **session-scoped, in-memory, never
invalidated** (the blob and descriptor set are immutable for the session's
lifetime, so a computed entry is valid forever once cached):

- **Global bounded cache**, keyed by the tag-stripped target range (§1):
  stores up to a fixed cap of top-ranked entries (exact cap is an
  implementation detail, tuned at implementation time — not fixed by this
  spec) per range, LRU-evicted across ranges once a bound on the number of
  distinct cached ranges is reached. Lets reopening a previously-viewed
  range's pane show its first screenful instantly.
- **Active-range cache**: a single slot holding the *complete* ranked list
  for whichever range the override pane currently targets, computed once
  per `t` invocation and discarded/replaced when the pane closes or
  retargets a different range. Supports free scrolling within an open
  pane without incremental slice-fetching logic.

**Lexicographic order** needs neither tier: it's the same list (all known
message/group type FQDNs, sorted) for every range, computed once and
cached as a single session-global list, independent of range.

**The pinned raw entry (§3.1)** needs no caching at all: it's not computed
by `score_all` or by sorting the type universe, just a static row prepended
to whichever list is currently displayed.

### §7 — Command-line prefix matching and Tab-completion

`run_command`'s dispatcher accepts any unambiguous prefix of a known
command name, with **exact match always winning over prefix ambiguity**:
typing a command's full name resolves to itself even when it's also a
prefix of another, longer command name — so `:type-as` resolves to itself,
not treated as ambiguous with `:type-as-raw`. This matches vim's own
`:command` abbreviation convention and `argparse`'s prefix-matching, not a
bespoke rule invented for this spec. Non-exact, still-ambiguous prefixes
(`:t`, `:ty` — both extend past the point where `:type-as`/`:type-as-raw`
diverge) are rejected with a status-line error listing the candidates;
`:e` unambiguously resolves to `:extract`.

This is what makes `:type-as`/`:type-as-raw` work cleanly as two full
command names rather than one command with a `--raw` flag (Goal 4/§5): the
earlier concern — that `:type-as` would be simultaneously a complete
command name and an ambiguous prefix of `:type-as-raw` — is resolved by
the exact-match rule, not by avoiding the name overlap.

Both the dispatcher's prefix-matching above and the command line's
Tab-completion (spec 0113 D26) are driven off the same single
source-of-truth command-name registry, so `:type-as`/`:type-as-raw`
require no bespoke completion code of their own — they participate
automatically once added to that registry. `:type-as`'s FQDN argument
additionally gets Tab-completion, once the command name itself has
resolved (D26): candidates are the same session-global, lexicographically-
sorted FQDN list §3.2/§6 already compute and cache, reused here rather
than recomputed.

### §8 — Key bindings (additions to spec 0111 Annex B)

Main pane (unchanged bindings not repeated):

| Key | Action |
|---|---|
| `t` | Open override pane for the message/group node under the cursor; toggles it closed if already open |

Override pane (only meaningful while it has focus, except `Tab`/`t` which work regardless of focus):

| Key | Action |
|---|---|
| `j` / `Down`, `k` / `Up` | Move highlighted candidate (or onto the pinned raw entry) |
| `i` | Toggle candidate sort: inferred score (default) / lexicographic — resets to `Inferred` every time the pane opens (see amendment below), not persisted across `t` invocations |
| `/` | Search forward |
| `?` | Search backward |
| `n` | Repeat last search |
| `Tab` | Move focus to the main pane (override pane stays open) |
| `Enter` | Apply highlighted candidate (or raw), close pane |
| `Esc` | Cancel, close pane |

Command line (while a `:` command is being typed — spec 0113 D21/D26):

| Key | Action |
|---|---|
| `Tab` | Complete the current token (command name, or `:type-as`'s FQDN argument) to the longest common prefix of matches; if already at that prefix, cycle forward through candidates |
| `Shift-Tab` | Cycle backward through candidates |

Commands (§7 — bypass the override pane entirely, applying to the cursor node):

| Command | Action |
|---|---|
| `:type-as <FQDN>` | Apply `<FQDN>` as the cursor node's type override |
| `:type-as-raw` | Mark the cursor node's range as explicitly raw/unschema'd |

No conflict with the override pane's own `Tab` (§2): command-line editing
and the override pane are mutually exclusive input modes — command-line
mode already fully shadows every normal-mode key binding (spec 0113 D21),
so the two `Tab` bindings never compete for the same keypress.

---

## Open Issues

1. **Cache sizing**: exact cap on cached entries per range and on the
   number of distinct ranges retained in the global bounded cache (§6) —
   tune at implementation time against realistic descriptor-set sizes, not
   fixed here.
2. **Minimum terminal width** below which `t` is refused (§2) — proposed to
   match 0111 Annex C's existing Phase-5 threshold (100 columns) for
   consistency, but not yet validated interactively.

---

## Files changed (anticipated)

| File | Change |
|---|---|
| `protolens/src/tui.rs` | Override pane state/rendering, focus toggle, key handling, splice-on-apply; `:type-as`/`:type-as-raw` registered in the shared command-name registry (spec 0113 D26); `App` gains a `wrapper_offset` field and pulls `blob`/`wrapper_offset` from `Decoded` instead of a separate constructor argument; `display_range` helper (half-open, payload-only, offset-corrected) backs the status line, override-pane title, and `default_extract_path`; `positional_path` drops the leading `/1` leg (§1.1) |
| `protolens/src/override_pane.rs` (new; `override` itself is a reserved Rust keyword, unusable as a module name — see Background's naming note) | Candidate-list computation, sort modes, caching (§6), `:type-as`/`:type-as-raw` command logic |
| `protolens/src/decode.rs` | Call `prototext_core::decode_and_render_indexed` for a sub-range re-render with `initial_level`/`emit_header` (spec 0110); build and register the synthetic wrapper `FileDescriptorProto` and prepend the wrapper tag+length prefix to the blob before initial decode (§1.1); `decode()`'s `ctx` parameter becomes `&mut DescriptorContext`; `Decoded` gains `blob`/`wrapper_offset` fields |
| `protolens/src/main.rs` | `ctx` becomes mutable to match `decode()`'s new signature; `App::new` call site simplified (blob no longer passed separately) |
| `prototext-core/src/serialize/render_text/sink.rs` | Add `NodeSpan::is_message: bool`, set unconditionally at both `IndexingTextSink` push sites (§1.2) |
| `protolens/src/extract.rs` | `is_message` computation switches from `type_fqdn.is_some()` to `span.is_message` (§1.2); `message_payload_range` becomes `pub(crate)` for reuse by `tui.rs`'s `display_range` (§1.1) |
