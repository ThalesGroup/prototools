<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0120 — protolens: Any/MessageSet expansion as automatic overrides

Status: implemented
Implemented in: 2026-07-14
Refs: docs/specs/0110-render-sink-unification.md,
      docs/specs/0113-protolens-tui-refinements.md,
      docs/specs/0114-protolens-range-type-override.md,
      docs/specs/0117-protolens-override-collection.md,
      docs/specs/0118-protolens-recursive-override-rendering.md,
      docs/specs/0119-protolens-override-fidelity-and-workflow.md
App: protolens

## Background

Interactive testing against a real `proxy.Sample` document (with
nested `Any`-typed fields) surfaced three linked bugs:

1. Down-arrow navigation jumps backwards through Any-expanded content, with
   wrong positional paths.
2. Overriding the document root with raw/no-type shows a stray `1` token in
   the header, surviving even after deactivating the override.
3. Reactivating a real type on the root after having gone raw loses
   Any/MessageSet expansion on descendants (renders as raw bytes with a
   bogus field number `0`).

Bugs 2 and 3 were root-caused and fixed independently (spec 0119-adjacent
work, `field_name_for`/`splice_override` header-stripping and the G1
natural-type fallback) and are not this spec's concern. Bug 1 is.

**First (wrong) diagnosis**: Any/MessageSet expansion looked like a
legitimate "virtual node" design (`Sink::virtual_scalar`/
`begin_virtual_nested`, spec 0110 §3) that merely needed a doc-order
tiebreak fix in `build_tree` (`decode.rs`) when a virtual node's
`raw_range.start` ties with its own first child's. That fix
(`(raw_range.start, level)` sort key) was implemented and tested, but did
**not** fix the reported symptom — down-arrow still skips `type_url`.

**Corrected diagnosis**, confirmed by reading
`prototext-core/src/serialize/render_text/helpers/any_field.rs` and
`message_set_field.rs` in full: Any/MessageSet expansion should never have
produced a virtual node in the first place. What it actually does today
(`render_any_expansion`, `any_field.rs:187-297`):

- Emits `begin_nested` for the outer Any-typed field — real field number,
  real range, fine.
- Emits `virtual_scalar("type_url", ...)` for field 1 — **produces no
  `NodeSpan` at all** (`IndexingTextSink::virtual_scalar` is delegate-only,
  `sink.rs:1243-1254`). This is why down-arrow skips over `type_url`: there
  is nothing to land on.
- Emits `begin_virtual_nested("value", ...)` for field 2 — produces a
  `NodeSpan`, but with a **hardcoded `field_number: 0`** and a range that
  starts *after* field 2's own real tag+length prefix (`value_payload_start`,
  not `raw_range.start`) — i.e. field 2's own wire framing is silently
  discarded, which is what caused the doc-order tie the first (wrong) fix
  patched over instead of removing.

`message_set_field.rs`'s `render_message_set_expansion` is analogous and
worse: every repeated `Item` wrapper node gets the **same** placeholder
range `0..data.len()` regardless of repetition (`message_set_field.rs:
328-331`, an acknowledged coarse approximation), so distinct repetitions of
a MessageSet item are indistinguishable by range at all.

**Investigated and disproved**: a suspicion that `prototext` CLI's own
`decode` path has independent Any/MessageSet-detection logic protolens
should instead mirror. Traced the call chain
(`prototext/src/run.rs` → `prototext_core::render_as_text` →
`serialize::render_text::decode_and_render`) against protolens's own
(`decode_and_render_indexed`) and confirmed both are literal sibling
functions (`render_text/mod.rs:243`/`:309`) sharing the exact same
`render_message` → `render_len_field` → `render_any_expansion`/
`render_message_set_expansion` chain, generic over `Sink` — differing only
in which `Sink` impl is active (`TextSink` vs `IndexingTextSink`). There is
no separate Any/MessageSet logic anywhere else to mirror.

**Chosen design** (this spec): Any/MessageSet expansion is not a rendering
concept at all — "it simply results in overriding what would otherwise be
`bytes` into a specific message" (verbatim framing from review). So:

- Disable `expand_any`/`expand_message_set` in protolens's own
  `DecodeRenderOpts` at both its call sites (`decode()`, `splice_override`)
  — **zero `prototext-core` changes**; `prototext` CLI's own defaults
  (`expand_any: true, expand_message_set: true`) are untouched, since each
  caller owns its own `DecodeRenderOpts` value.
- With expansion disabled, `render_len_field` (`len_field.rs`) falls
  through to paths that are already correct and already used elsewhere:
  - An Any-typed field is schema-known (`Kind::Message(any_desc)`, `any_desc`
    being `google.protobuf.Any`'s own real 2-field descriptor:
    `type_url` string field 1, `value` bytes field 2) — ordinary
    `begin_nested`/`render_message` path (`len_field.rs:209-221`). Both
    `type_url` and `value` get real, correctly-ordered `NodeSpan`s "for
    free," via the same `scalar_field` logic every other field uses
    (`sink.rs:1087-1172`).
  - A MessageSet-typed field (`message_set_wire_format = true`, zero
    declared fields) falls to the unknown-LEN-field cascade
    (`len_field.rs:52-95`, spec 0097): each repeated group occurrence is
    probed and recursively rendered independently, giving each `type_id`
    (field 2, varint)/`message` (field 3, bytes) pair, inside each repeated
    group item (field 1), real per-repetition `NodeSpan`s — fixing the
    pre-existing "every Item shares `0..data.len()`" imprecision as a
    byproduct.
- protolens itself detects, during its existing `render_overrides`
  recursive pass (`tui.rs`), when a node is Any/MessageSet-*shaped*, and
  automatically computes-and-applies (via the existing, unmodified
  `splice_override`) an override on the relevant descendant node — exactly
  what a user would do by hand with `t`/`:type-as`, just automatic
  (seeded as a real, visible `OverrideEntry` the first time it's derived —
  see "Non-goals" below for why this diverges from the plan as originally
  drafted here). An explicit user override on that same node always takes
  priority.

This reuses `splice_override` (`tui.rs:1447-1691`) entirely unmodified: it
is already fully generic (extracts `payload_range` purely from
`old_span.raw_range`/`packed_record_start`, resolves the target via
`ctx.pool().get_message_by_name`, has no dependency on `is_message` or any
other property of the node being re-spliced) and reuses tested machinery
rather than adding a new mechanism.

### Recursion-complexity assessment (requested before drafting this spec)

The two shapes are structurally different and need separate handling —
this is the "intervening deeper in the nesting stack" complexity flagged
during review:

**Any** — one hop up, one hop sideways, from the *target* node's own
perspective. When `render_overrides` visits a node `idx` with
`field_number == 2` whose *parent* has `type_fqdn == Some("google.protobuf.Any")`,
the auto-derived target type is read from the sibling `field_number == 1`
node's own decoded string value.

**MessageSet** — two hops up, one hop sideways. When `render_overrides`
visits a node `idx` with `field_number == 3` whose *parent* is an
unknown/untyped node (`type_fqdn: None` — the auto-generated group-item
wrapper) whose own *parent* resolves (via `ctx.pool()`) to a
`message_set_wire_format`-flagged, zero-field descriptor, the auto-derived
target type is resolved as an *extension* of that grandparent's type,
keyed by the sibling `field_number == 2` node's own decoded varint value
(`extendee.get_extension(type_id)` — the exact lookup
`install_any_loader`'s existing MessageSet-path closure already performs,
`decode.rs:457-471`).

Both fit into `render_overrides`'s **existing** per-node visit and
**existing** sibling/child recursion (`tui.rs:1381-1427`) with no new
looping/pre-pass structure:

- `render_overrides` already recurses into every `is_message` child of a
  node in turn (its own `next_sibling` walk) — so for MessageSet, every
  repeated group-item child of the container is already visited on its own,
  one at a time, without any new repetition-handling code. The per-item
  `message`/`type_id` pair only needs a **single node's** two-hop-up check,
  not a lookahead from the container.
- The auto-derived type slots into the exact same fallback position
  `natural_type` already occupies (spec 0119 §G1): explicit active
  override > auto-derived Any/MessageSet type > `natural_type` > raw.
  `natural_type` already correctly returns `None` for both `value` (`Kind::
  Bytes`, not `Message`) and `message`/`Item` (no parent schema at all,
  since these are unknown-cascade fields) — no interaction/precedence
  conflict, the two fallbacks are mutually exclusive in practice.
- Nested Any/MessageSet (an Any's `value`, once auto-overridden to a
  concrete type, itself containing another Any-typed field) resolves for
  free: `splice_override` decodes the new subtree fresh (still under
  `expand_any: false`), and `render_overrides`'s own recursive walk visits
  the newly-spliced children next, re-running the same per-node detection
  uniformly — no separate recursive-detection pass is needed.

**Value/varint reading**: neither shape needs new low-level plumbing.
`extract::message_payload_range` (`protolens/src/extract.rs:76-98`,
already used by `splice_override`) already strips a scalar node's own tag
(+length, for `WT_LEN`) generically, regardless of wire type — reused
as-is to get `type_url`'s raw UTF-8 payload and `type_id`'s raw varint
payload. `prototext_core::helpers::parse_varint` (already imported by
`decode.rs`, fully `pub`) decodes the varint. No new dependency, no
`NodeSpan`/`Sink`/`IndexingTextSink` change.

**MessageSet-option query**: confirmed protolens does **not** need
`NodeSpan`/`IndexingTextSink` to convey `message_set_wire_format` at all.
protolens already depends directly on `prost_reflect` and already performs
equivalent pool lookups elsewhere (`parent_field`/`natural_type`, spec
0119; `install_any_loader`, `decode.rs:452-476`). The exact same query
`prototext-core`'s own (private, `pub(in super::super)`) `is_message_set`
uses (`message_set_field.rs:29-37`) —
`desc.descriptor_proto().options.as_ref().and_then(|o|
o.message_set_wire_format).unwrap_or(false)`, plus `desc.fields().count()
== 0` — is directly callable by protolens on a `MessageDescriptor` obtained
from its own `ctx.pool().get_message_by_name(fqdn)`, with no crate-boundary
issue. This resolves review caution #2 outright: no `IndexingTextSink`
plumbing is needed.

## Goals

- G1: Any-typed fields (`type_fqdn == Some("google.protobuf.Any")`) are
  recognized by protolens via FQDN match (per review caution #1). The
  `value` field (field number 2) is automatically overridden to the
  concrete message type named by the sibling `type_url` field's own
  decoded string (the FQDN is the substring after the last `/` — mirrors
  `any_field.rs`'s own `fields.type_url.rfind('/')` resolution,
  `any_field.rs:209-212`), whenever that type is resolvable in
  `ctx.pool()`. An unresolvable `type_url` leaves `value` rendered as
  plain bytes — no error, matches today's raw-fallback behavior elsewhere
  (e.g. `natural_type`'s own miss case).
- G2: MessageSet-typed fields are recognized by protolens via the
  `message_set_wire_format` option (per review caution #2), queried
  directly off the resolved `MessageDescriptor` — no `NodeSpan`/
  `IndexingTextSink` changes (per the assessment above). Each repeated
  group item's `message` field (field number 3) is automatically
  overridden to the concrete extension message type resolved from the
  sibling `type_id` field's own decoded varint value, treated as an
  extension field number on the MessageSet-typed container
  (`extendee.get_extension(type_id)`), whenever resolvable. An
  unresolvable `type_id` leaves `message` rendered as plain bytes.
- G3: cursor navigation (up/down/all movement) traverses Any/MessageSet-
  expanded content in correct document order, indistinguishable from
  navigating into a manually-overridden field of the same shape (fixes
  Bug 1). Falls out of G1/G2 for free — no `NodeSpan`/doc-order special
  casing survives this spec; `splice_override`/`build_tree` are already
  correct for manual overrides.
- G4: reactivating a real schema type on an ancestor after a raw/no-type
  override reliably re-derives Any/MessageSet auto-expansion on every
  affected descendant (fixes Bug 3, if it still reproduces once G1/G2 are
  in place — expected to, since `splice_override` always performs a
  genuine fresh recursive decode+`render_overrides` pass, but should be
  re-verified with a regression test under the new design rather than
  assumed).

## Non-goals

- Any change to `prototext-core` (`any_field.rs`, `message_set_field.rs`,
  `sink.rs`'s `virtual_scalar`/`begin_virtual_nested`, `NodeSpan` shape) —
  all remain exactly as-is. `prototext` CLI's own `expand_any`/
  `expand_message_set` defaults are untouched.
- ~~Persisting the auto-derived Any/MessageSet override as a real
  `OverrideEntry`~~ — **superseded during implementation**: the
  auto-derived type *is* seeded as a real `OverrideEntry` (`OverrideOrigin
  ::Path`, `auto: true`) the first time its origin is visited with no
  existing entry, deliberately, so it shows up as a normal, visible,
  user-editable/removable/renameable row in the manage pane rather than a
  silent dynamic fallback — and, since `to_yaml`/`from_yaml` make no
  distinction based on `auto`, it *does* round-trip through the YAML
  save/restore format like any other entry. The `auto: bool` field is what
  keeps this safe: it marks the entry as re-derivable rather than
  user-endorsed, so a subsequent `render_overrides` pass can *demote* it
  (silently stop honoring it for this pass, without touching `active`)
  if its governing ancestor's context changes such that the original
  derivation no longer holds — see "Demotion" in the `render_overrides`
  doc comment (`tui.rs:1556-1592`). Any explicit user action on the entry
  (`activate`, a manual `t`/`:type-as`) pins `auto` back to `false`,
  making it behave exactly like any other user-authored entry from then
  on. This is a stronger, more consistent design than the originally
  planned silent non-persisted fallback (spec 0119 §G1's `natural_type`
  precedent it was meant to mirror stays non-persisted, since it has no
  analogous demotion need).
- Changing how an explicit, user-authored override on the same node
  (Any's `value` / MessageSet's `message`) behaves — it always takes
  priority over the auto-derived type, same priority rule as
  `natural_type`.
- Any/MessageSet values inside a packed-repeated record (spec 0115) — out
  of scope; Any/MessageSet fields are always singular or (for MessageSet
  items) group-encoded, never a packable scalar kind, so this combination
  cannot arise.
- Re-litigating Bugs 2/3's already-applied, independent fixes (root header
  stripping in `field_name_for`/`splice_override`; G1's `natural_type`
  fallback) — kept as-is, though the two Phase-2 fixes specific to the old
  virtual-node design (`build_tree`'s `(raw_range.start, level)` doc-order
  tiebreak; `render_overrides`'s `field_number == 0` skip-resplice guard)
  are addressed under "Cleanup" below since they target a design this spec
  removes.

## Specification

### Detection helpers (`tui.rs`)

- `fn is_any_typed(&self, idx: usize) -> bool` — `self.tree[idx].span
  .type_fqdn.as_deref() == Some("google.protobuf.Any")`.
- `fn is_message_set_typed(&self, idx: usize) -> bool` — resolve
  `self.tree[idx].span.type_fqdn` via `ctx.pool().get_message_by_name`,
  then check `message_set_wire_format` + zero declared fields, mirroring
  `prototext-core`'s own `is_message_set` (not callable directly — it's
  `pub(in super::super)`, private to that crate — so this is a small
  independent replica in protolens, not a shared helper. This asymmetry
  is intentional and low-risk: both sides read the same well-known
  `MessageOptions` field via the same `prost_reflect` API, and a genuine
  future drift between the two copies is limited to this one small
  predicate).

### `auto_expand_type(&self, idx: usize) -> Option<String>` (`tui.rs`)

New fallback tier, consulted in `render_overrides` between the explicit
active override and `natural_type`:

```
match resolve_active_override(idx) {
    Some(explicit) => explicit,
    None => auto_expand_type(idx).or_else(|| natural_type(idx)),
}
```

Implementation, by `idx`'s own `field_number` and one/two hops up its
parent chain:

- `field_number == 2` and parent `is_any_typed`: find the sibling with
  `field_number == 1` (walk parent's `first_child`/`next_sibling` chain);
  read its raw payload via `extract::message_payload_range` (tag-only
  strip, `WT_LEN`), decode as UTF-8, resolve FQDN as the substring after
  the last `/`; return `Some(fqdn)` iff `ctx.pool().get_message_by_name
  (&fqdn)` resolves, else `None`.
- `field_number == 3`, parent has `type_fqdn: None` (unknown-cascade group
  item), and grandparent `is_message_set_typed`: find the sibling with
  `field_number == 2` under the same parent; read its raw payload via
  `extract::message_payload_range` (tag-only strip, `WT_VARINT`), decode
  via `prototext_core::helpers::parse_varint`; return
  `Some(extension_desc.full_name().to_string())` iff
  `ctx.pool().get_message_by_name(grandparent_fqdn)?.get_extension
  (type_id as u32)` resolves to a message-kind extension, else `None`.
- Any other `idx`: `None`.

**MessageSet tier 1, added during implementation (not anticipated above)**:
the repeated group-item wrapper itself (field 1, `WT_START_GROUP`) also
gets auto-overridden — to a synthetic, pool-wide-registered two-field
message type `protolens_internal.MessageSetItem` (`type_id` varint field
2, `message` bytes field 3; `decode::register_message_set_item`,
registered once per pool and reused by every MessageSet occurrence in the
document). This turns the wrapper from an unknown-cascade node into a
genuinely `is_message` one with real schema-declared field names, which is
what lets tier 2's `message` field resolve its own parent/grandparent
chain (`is_message_set_typed(grandparent)`) the same way regardless of how
deep the MessageSet container is nested — and gives the wrapper a display
name at all, since the unknown-cascade path has no schema name to draw on
(seeded as `"Item"` via spec 0119 §G4's per-entry `name` override,
matching `prototext-core`'s own native-rendering label for this wrapper).

### `render_overrides` changes (`tui.rs:1381-1427`)

- Drop the `span.field_number != 0` skip-resplice guard entirely (see
  "Cleanup" below) — with expansion disabled, no node this spec's design
  produces ever has a fabricated `field_number: 0` (the only remaining
  legitimate `field_number: 0` case is `Sink::malformed`'s
  `InvalidTagType`, per `NodeSpan`'s other documented meaning, spec
  0110).
- Widen the effective-type computation (currently `target.clone()` or
  `natural_type(idx)`) to the three-tier chain above.
- The child-recursion gate widens from `span.is_message` alone to
  `span.is_message || is_auto_expand_candidate(idx)` (as implemented — the
  original assumption above that the gate "stays `span.is_message`" turned
  out wrong): Any's `value` (field 2) and MessageSet's `message` (field 3)
  are still plain scalar bytes at the moment `render_overrides` first
  visits them, so a bare `is_message` gate never reaches them at all —
  they only become `is_message` *after* being auto-overridden, i.e. one
  pass too late. `is_auto_expand_candidate` is a narrow structural
  predicate (matches only these two field shapes, by field number and
  parent/grandparent type) so this widening cannot reopen the spec 0119
  bug where every plain scalar LEN-wire field got demoted to a raw record
  dump by being recursed into unconditionally.

### Cleanup: removal of the Phase-2 virtual-node-specific fixes (done)

Both were correct, tested fixes for the old (now-removed) virtual-node
design and became dead weight/misleading once that design was gone; both
were resolved as follows:

- `decode.rs`'s `build_tree`: the `(raw_range.start, level)` doc-order
  tiebreak was reverted precisely to plain `raw_range.start` — every
  `NodeSpan` now has a real, non-shared tag/length prefix, so no tie
  condition survives (`decode.rs:310-316`, comment updated to explain why
  ties can no longer occur rather than describing the removed virtual-node
  case).
- `decode.rs`'s test `build_tree_orders_virtual_any_wrapper_before_its_own_
  children` and `decode_expands_any_fields_via_installed_loader`: removed
  (their premise, a virtual wrapper node, no longer exists).
- `tui.rs`'s `render_overrides`: the `span.field_number != 0` guard and its
  explanatory comment were removed (superseded by the three-tier fallback
  chain and the `auto`-flag demotion mechanism above).
- `tui.rs`'s `splice_override_reactivating_root_type_still_expands_any_
  fields`: rewritten against the new auto-override mechanism's actual
  rendered output (asserts the root can be retyped raw and back to a real
  type without losing `Any` expansion on descendants — this is G4's
  regression coverage for the Any shape).
- `install_any_loader`/`clear_any_loader`/`ANY_LOADER` (formerly
  `decode.rs:452-476`): removed entirely, resolving the leaning stated
  when this spec was drafted — dead code once `expand_any`/
  `expand_message_set` are `false` at both call sites.

## Open Issues (resolved)

- `build_tree`'s doc-order tiebreak was reverted precisely to
  `raw_range.start` alone (no `level` secondary key) — see "Cleanup."
- `install_any_loader`/`clear_any_loader` and their call sites were
  removed entirely — see "Cleanup."
- `auto_expand_type`'s FQDN/extension-resolution logic was kept as
  separate small helpers (`is_any_typed`, `is_message_set_typed`,
  `find_sibling`, `read_string_field`, `read_varint_field`) rather than
  collapsed into one function — this reads more clearly given the
  MessageSet path's extra tier-1 synthetic-type step (see
  "Specification" above), which the original draft didn't anticipate.
- G4 (MessageSet auto-expansion re-derivation after ancestor retype) now
  has a dedicated fixture (`message_set_fixture`, `tui.rs`) and regression
  tests (`message_set_group_items_auto_expand_through_render_overrides`,
  `toggling_message_set_auto_override_off_and_on_sticks`,
  `deactivating_tier_1_does_not_affect_the_still_active_tier_2_entry` —
  the last one originally covered the `auto`-flag demotion mechanism;
  see the 2026-07-17 addendum below, which removed demotion and
  rewrote/renamed this test to match).

## Addendum, 2026-07-17: demotion removed

The "Demotion" mechanism described above (`Non-goals`' superseded bullet;
`auto: bool`'s original doc comment; `render_overrides`'s per-pass
re-check of a stale `auto` entry's governing ancestor) has been removed.

User feedback (interactive session, 2026-07-17): deactivating a
MessageSet's tier-1 (`Item`) auto-derived override was silently making
its tier-2 (`message`) auto-derived override stop applying too, even
though tier-2's own entry was never touched and stayed `active`. This
was demotion working exactly as originally designed, but the design
itself was judged confusing: "In my mental model auto/manual overriding
and the memorization of it is only for showing 'provenance' to the
user. With respect to rendering, an override is an override. It can be
active or inactive. However, its provenance should have no impact
whatsoever on how the protobuf is rendered."

Change: `auto` is now pure provenance (how an entry was created, shown
via `manage_entry_style`'s color), with zero effect on override
resolution. `resettle_node` (formerly gated through a since-removed
`effective_override_target` demotion check) now always resolves via
`resolve_active_override` directly — an active entry, auto-derived or
not, applies exactly as long as its path still resolves to a live node,
same as a manual override always did. If an ancestor's retype makes a
descendant's stored type structurally incompatible with the actual
bytes, the existing `TYPE_MISMATCH`-style fallback in `splice_override`
handles it, same safety net manual overrides already relied on — no
separate demotion mechanism is needed.

`auto_entry_in_scope` (the "would this auto entry still be re-derived
if visited again right now" check) is kept, but narrowed to its one
remaining use: the manage pane's Delete/Backspace key handling, which
still needs to distinguish "deleting this in-scope auto entry would
just be immediately re-seeded, so deactivate instead" from "this entry
is out of scope, delete it for real." This is a UX convenience for the
delete action itself, not a render-time behavior.

The regression test that had codified demotion,
`deactivating_tier_1_demotes_the_still_active_tier_2_entry`, was
rewritten and renamed to
`deactivating_tier_1_does_not_affect_the_still_active_tier_2_entry`,
asserting the opposite: tier-2 keeps applying (and its entry stays
active) regardless of tier-1's own active state.

## Files changed

- `protolens/src/decode.rs` — `decode()`'s `DecodeRenderOpts` literal
  (`expand_any: false, expand_message_set: false`); `build_tree`
  doc-order tiebreak reverted to plain `raw_range.start`; removal of
  `install_any_loader`/`clear_any_loader`/`ANY_LOADER` and their call
  sites; new `MESSAGE_SET_ITEM_FQDN`/`register_message_set_item` (tier-1
  synthetic type registration); test updates per "Cleanup."
- `protolens/src/tui.rs` — `splice_override`'s `DecodeRenderOpts` literal
  (same two fields); new `is_any_typed`/`is_message_set_typed`/
  `is_auto_expand_candidate`/`find_sibling`/`read_string_field`/
  `read_varint_field`/`auto_expand_type` helpers; `render_overrides`'s
  fallback-chain widening, auto-seeding-as-`OverrideEntry`, and demotion
  logic; `splice_override`'s `wire_type`-preservation fix (`tui.rs:1905`,
  needed so `is_auto_expand_candidate`'s `span.wire_type` reads stay
  accurate across repeated splices); `field_number != 0` guard removal;
  test updates per "Cleanup," plus new MessageSet fixture/regression
  tests (G2/G4).
