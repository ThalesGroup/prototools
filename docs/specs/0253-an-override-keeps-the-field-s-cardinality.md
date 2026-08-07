<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0253 — an override keeps the field's cardinality

Status: implemented
Implemented in: 2026-08-07
App: protolens
Refs: docs/specs/0135-protolens-override-raw-tag-rewrap.md (the synthetic
        one-field wrapper an override renders through, and its
        `"_"` placeholder name),
        docs/specs/0219-a-length-delimited-record-can-be-read-as-a-packed-run.md
        (`packed_framing`, the existing single authority on
        `repeated [packed=true]`),
        docs/specs/0119-protolens-override-fidelity-and-workflow.md
        (`parent_field`, the parent-schema lookup this reuses),
        docs/specs/0249-a-large-document-answers-the-user-first.md (S8,
        where the defect surfaced and is recorded as a stated limit)

## Background

Overriding a node's type silently drops the `repeated` / `required`
qualifier its own field carries in the parent's schema. Measured on a
two-level fixture whose `items` field is declared `repeated Item`, taking
one element's header line before the splice, after a splice to the same
type, and after a splice to raw:

```
before  "  items {  #@ repeated Item = 1"
 after  "  items {  #@ Item = 1"
   raw  "  1 {  #@ message"
```

The cause is one line. An override renders the node's own bytes through a
synthetic one-field wrapper message (spec 0135), and
`decode::register_wrapper` declares that field's label from the *wire
framing* alone:

```rust
label: Some(if packed { Label::Repeated } else { Label::Optional } as i32),
```

`packed` is `decode::packed_framing(&span)` — "these bytes are framed as a
LEN record on a packable primitive" (spec 0219 S3). That is the right rule
for `[packed=true]`, and the only rule there was. Nothing ever consults the
parent's schema, even though the node has a perfectly good field
declaration there and `App::parent_field` (spec 0119) already reaches it.

Two things read the label, so this is not only cosmetic:

- `prototext-core`'s annotation writers,
  `render_text/helpers/annotations.rs:69` and `:185`, emit `"required "` /
  `"repeated "` / `""` straight from `fi.cardinality()`. This is the
  visible defect above.
- `render_text/helpers/len_field.rs:137` gates the packed-run decode on
  `fs.cardinality() == Cardinality::Repeated`.

And protolens already disagrees with itself: `export --descriptor` derives
the same label the right way, from the same node, at
`tui/override_export.rs:57`:

```rust
let is_repeated = parent_field_desc
    .as_ref()
    .map(|f| f.cardinality() == Cardinality::Repeated)
    .unwrap_or(false)
    || g.count > 1;
```

So the descriptor protolens writes out says `repeated` for a node whose
header on screen says nothing.

## Goals

- **G1.** An overridden node's header carries the same `repeated ` /
  `required ` qualifier the un-overridden node carried, whenever the
  parent's schema declares the node's field. This covers user overrides
  (`:override`, the override pane) and the automatic ones (Any / MessageSet,
  spec 0120) alike — they are the same splice.
- **G2.** One authority for the wrapper's label, consulted by both the
  splice and the override pane's warming pass, so warming cannot register a
  wrapper the splice then never looks up (spec 0219 S4's rule, which today
  holds only because both sites call `packed_framing`).
- **G3.** The label the header shows and the label
  `export --descriptor` writes agree for every node whose field is declared.

## Non-goals

- **N1.** No user-settable cardinality. `:override` gains no
  `--cardinality` flag and the override file gains no field. Cardinality is
  *read* from the document's schema, never chosen — the user is retyping a
  node, not redeclaring its parent.
- **N2.** No guess when there is no parent schema. This is the "(if any)"
  case: an unknown field, a raw parent, or a parent whose type is
  unresolved keeps `Optional` and shows no qualifier. In particular
  `override_export`'s `|| g.count > 1` occurrence-count fallback is *not*
  imported into the display — see Alternatives.
- **N3.** No change to how a packed run is detected, rendered or
  overridden. `packed_framing` stays the single authority on
  `[packed=true]`; this spec only stops it being the single authority on
  `repeated` as well.
- **N4.** The root override is untouched. `render_resolved` registers
  wrapper field 1 for the whole document, which has no parent and therefore
  no cardinality to inherit; it stays `Optional`.

## Specification

- **S1. The wrapper's label is an input, not a derivation.**
  `decode::register_wrapper` takes the label from its caller instead of
  computing it from `packed`. It keeps exactly one rule of its own, and
  that rule is a protobuf requirement rather than a preference: **a packed
  field must be repeated**, so a `packed` wrapper is forced to
  `Label::Repeated` whatever the caller asked for. (`packed` is itself
  already `packed_framing && is_packable(field_type)`, so this only fires
  where protobuf permits packing at all.)

- **S2. The label comes from the parent's field, or is `Optional`.** One
  new accessor on `App`, next to `parent_field`, is the single place the
  rule lives:

  - `parent_field(idx)` resolves and its `cardinality()` is `Repeated` →
    `Label::Repeated`;
  - … `Required` → `Label::Required`;
  - … `Optional`, or `parent_field(idx)` is `None` → `Label::Optional`.

  `Required` is legal here because `register_synthetic` declares the
  synthetic file `syntax: "proto2"` (`decode.rs:1408`). Under proto3 it
  would be rejected by `add_file_descriptor_proto`, and the whole
  `required` half of this spec would be unreachable.

- **S3. The label enters the wrapper's name.**
  `synthetic_wrapper_name` hashes `"{field_number}:{type_str}:{type_name}"`
  plus `":packed"`, and that hash is the pool key that
  `register_wrapper`'s early `get_message_by_name` return depends on.
  Adding a third label state without adding it to the key makes two
  genuinely different wrappers — field 1 `optional int32` and field 1
  `required int32` — collide on whichever was registered first, and the
  second node renders under the first's declaration.

  The label is appended the same way `packed` is, and only when it is not
  `Optional`, so no existing wrapper name changes. These names are
  session-local pool entries: nothing persists them (the only
  `protolens_internal` string that reaches an override file is the
  `protolens_internal.None` raw sentinel), so there is no compatibility
  question.

- **S4. Both wrapper sites ask the same question.**
  `render_node_as` (`tui/override_apply.rs`) already calls
  `self.parent_field(idx)` a few lines above the registration, for the
  extension-bracket header name; it computes the label there. The override
  pane's `warm_visible_override_wrappers`
  (`tui/override_select.rs:593`) computes it from the same `idx` — it
  already holds `self.override_target` and already mirrors `packed` for
  exactly this reason. Both then pass it to `register_wrapper`, so warming
  and the splice keep hashing to the same name.

- **S5. The renderer is not touched, and the packed path does not move.**
  Setting `Repeated` on a wrapper field could in principle reach
  `len_field.rs:137`'s packed-run branch, but cannot: that branch also
  requires `is_packable_kind`, and it is only reached for a record whose
  *wire* type is LEN. A packable primitive with LEN framing is precisely
  `packed_framing`, which S1 already forces to `Repeated` *and* stamps with
  `packed: Some(true)` today. So every combination the change newly makes
  reachable — a repeated message, a repeated `string`/`bytes`, a repeated
  primitive whose element is not LEN-framed — either fails
  `is_packable_kind` or never reaches `render_len_field`. The change is
  confined to the annotation writers.

- **S6. `override_export` keeps its own derivation.** It agrees with S2 for
  every declared field, which is G3, but it also has to name a label for a
  field with no schema at all — that is what `|| g.count > 1` is for, and
  folding the two into one helper would drag that guess into the display,
  which N2 forbids. The two are left as two, and this spec is the note
  saying so.

- **S7. Spec 0249 S8's stated limit is retired.** Its "the expanded header
  can lose a `repeated` qualifier" paragraph, and the deliberate
  `assert_ne!` sentinel in
  `opening_an_auto_fold_renders_the_body_it_stood_for` that exists to fire
  when this is fixed, both go: the test compares the expanded header
  against the unbounded one exactly.

## Alternatives considered

### Patch the qualifier into the rendered header afterwards

Spec 0135 G2 already patches the field *name* into the header as a
post-render substring replacement, so the machinery exists. Rejected: it
would leave the descriptor in the pool still lying, so anything reading the
wrapper rather than the text stays wrong, and the qualifier appears in two
places on a header line (the annotation and, via `field_decl`, elsewhere) —
each needing its own anchored patch, each with spec 0143's `TYPE_MISMATCH`
class of false match to avoid. The label is a property of the declaration;
declaring it correctly costs one field.

### Derive the label from how many siblings the document happens to contain

This is `override_export`'s `g.count > 1`. Rejected for the display: it
makes the qualifier a fact about the blob rather than about the schema, so
a `repeated` field carrying exactly one element would render as optional,
and the same schema would annotate differently in two documents. Export has
no choice — it must emit a label for a field nothing declares — but the
display does, and "say nothing" is the honest answer there.

### Add a `--cardinality` flag to `:override`

Rejected as scope with no demand behind it. The reported defect is that the
override *drops* information the document already has, not that the user
wants to supply information it lacks. A flag would also have to be
persisted in the override file and reconciled with `packed`, for a case
nobody has asked for.

## Test plan

1. `an_override_keeps_the_repeated_qualifier` — splice a repeated element
   to its own type; the header still reads `#@ repeated Item = 1`. This is
   the measured regression in Background.
2. `an_override_keeps_the_required_qualifier` — same, on a proto2 fixture
   field declared `required`; establishes that the proto2 synthetic file
   really does accept the label rather than erroring at registration.
3. `an_optional_field_gains_no_qualifier` — the status quo does not
   regress into emitting `optional `.
4. `an_override_under_a_raw_parent_has_no_qualifier` — N2's case:
   `parent_field` returns `None` and the header carries no qualifier.
5. `two_cardinalities_are_two_wrappers` — register field 1 / `int32` twice
   with different labels and assert two distinct pool messages, each with
   the label asked for. Without S3 the second returns the first.
6. `warming_registers_the_wrapper_the_splice_looks_up` — warm a repeated
   node's candidates, then splice, and assert the splice found the wrapper
   already registered. Guards G2 directly; spec 0219 S4's invariant has no
   test today.
7. `a_packed_run_still_renders_packed` — an existing packed-run override
   test, unchanged, as the S5 regression guard.
8. `opening_an_auto_fold_renders_the_body_it_stood_for`
   (`tui/tests/navigation.rs`) — its header comparison drops the
   `replace("repeated ", "")` and its `assert_ne!` sentinel, per S7.

## Measured outcome

The defect is gone on the real corpus. Overriding
`InstanceGroupManagersScopedList`'s repeated `instance_group_managers`
field to its own type (`--load-overrides`, headless `export /` against
the 25.6 MB `googleapis.desc`) now renders

```
instance_group_managers {  #@ repeated InstanceGroupManager = 214072592
```

where before the override dropped the qualifier.

The change touched six lines of production code plus a doc comment each,
across four call sites: `register_wrapper` and `synthetic_wrapper_name`
(`decode.rs`), `App::field_cardinality` (`tui/override_resolve.rs`), and
the three registrations — the splice (`tui/override_apply.rs`), the
warming pass (`tui/override_select.rs`) and the root wrapper
(`decode.rs`'s `render_resolved`, pinned to `Optional` by N4).

**Nothing else moved.** A plain document render is byte-identical: the
only wrapper a render with no override in play builds is the root's, and
`Optional` appends nothing to its name. Test plan item 7's existing
packed-run test passed unchanged, confirming S5's argument that the
`len_field.rs` branch is unreachable from the newly permitted
combinations.

The one test that had to change is the one written to demand it: spec
0249 S8's `assert_ne!` sentinel fired with
`left == right == "  items {  #@ repeated Item = 1"`, and its header
comparison folded back into the whole-slice `assert_eq!`.

Workspace green: 774 protolens tests (up 7), `cargo clippy
--workspace --all-targets` clean, `cargo fmt --check` clean, `reuse
lint` 811/811.
