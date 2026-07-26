<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0181 — delete `NodeSpan::natural_annotation`

Status: implemented
Implemented in: 2026-07-26
App: prototext-core, protolens
Refs: docs/prototext/decode-flaws.md (P2),
      docs/protolens/rendering-scaling-roadmap.md (S12),
      docs/protolens/rendering-worklist.md (W25 step 5),
      docs/specs/0122-protolens-override-header-patching.md (§1),
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (Non-goals),
      docs/specs/0173-score-all-and-render-hot-path-allocations.md (N5)

## Background

`NodeSpan::natural_annotation`
(`prototext-core/src/serialize/render_text/sink.rs:1018`) is an
`Option<String>` holding the `#@ ...` annotation text a container node's
header line was rendered with. **Nothing reads it.**

A repo-wide grep finds 55 hits across 17 files and not one is a
production read:

- one computing site, `sink.rs:1272`, in `end_nested`;
- two `: None` literals in the same file (`:1189` scalar, `:1208` packed
  element);
- three `: None` initializers in `protolens/src/extract.rs`;
- five `prototext-core` tests that assert the field's own value
  (`render_text/mod.rs:917-1087`), and eight `: None` lines in
  protolens's test fixtures;
- a stale doc comment at `protolens/src/tui/tests/override_apply.rs:199`
  referring to an `.expect()` that no longer exists.

### Why it is still here: a documented claim that is false

Spec 0122 added the field for override header patching. Spec 0135
deleted that patching — the ~70-line `patched_annotation` token-splicing
block — which removed the only reader. Spec 0135's Non-goals then
declined to delete the field:

> `NodeSpan::natural_annotation` (`prototext-core`) itself is
> `pub`/general-purpose and used elsewhere in `prototext-core`
> (unrelated to this override-specific patching) — left untouched; only
> its *use* in `splice_override` is removed.

**There is no such use elsewhere.** The only other `prototext-core`
references are the five tests that exist to test the field. Spec 0135
correctly identified that it was deleting the last consumer, and then
declined to delete the producer on the strength of an unchecked
assumption — which is why this survived a later review that was looking
straight at it.

This spec **reverses that non-goal, on the basis that its stated reason
was wrong.** That is the whole of the justification needed; there is no
reader to weigh against.

### What it costs

`NodeSpan` is 120 B and `TreeNode` is 280 B. At the measured density of
0.566 nodes/byte, a 24.5 MB descriptor set produces ~13.9 M nodes, so
the arena is ~3.9 GB fresh and ~16.9 GB after two override commits.
The `Option<String>` is 24 B of every `NodeSpan` — **~330 MB** of that —
before counting the heap allocation, which `end_nested` performs once
per *container* node whenever annotations are on.

Note the correction to the original write-up recorded in decode-flaws
P2: "three producer sites" overstated it. Two of the three are `None`
literals; there is exactly one site that computes anything.

## Goals

- **G1** — `NodeSpan` loses the field, and every site that writes it
  loses the line. No production behavior changes, because no production
  code reads it.
- **G2** — the machinery that exists *only* to feed it goes with it:
  `natural_annotation_from` and `IndexMark::header_start`.
- **G3** — spec 0135's non-goal is recorded as reversed, in the spec
  itself, so the next reader finds the correction where the false claim
  is.
- **G4** — W25 step 5 changes from "intern `natural_annotation`" to
  "remove it", which is strictly less work and strictly more saving.

## Non-goals

- **N1** — the rest of the `TreeNode`/`NodeSpan` shrink (S12 / W25).
  This spec takes the one row of that table that is free because it has
  no reader; every other row is a real design change with a real
  consumer to satisfy.
- **N2** — interning or otherwise deduplicating `type_fqdn`, the other
  `Option<String>` on `NodeSpan`. It *is* read, in several places, and
  is a separate problem with a separate answer.
- **N3** — any change to the rendered text. `natural_annotation_from`
  is a pure forward scan over an already-written buffer; removing it
  cannot alter a byte of output. The `#@` annotations themselves are
  untouched — they remain in the rendered text, which is where they are
  actually consumed.
- **N4** — changing `DecodeRenderOpts::annotations` or the annotation
  rendering path in any way.

## Specification

### S1. `prototext-core/src/serialize/render_text/sink.rs`

Delete:

- the `natural_annotation` field on `NodeSpan` (`:1007-1018`, doc
  comment included);
- `natural_annotation_from` (`:1044-1061`, doc comment included);
- the `header_start` member of `IndexMark` (`:1036-1040`) and its three
  write sites (`:1229`+`:1247` in `begin_nested`, `:1310`+`:1324` in
  `begin_virtual_nested`) and its destructuring in `end_nested`
  (`:1266`);
- the computing call at `:1272` and the field's use at `:1282`;
- the two `natural_annotation: None` literals at `:1189` and `:1208`.

`header_start` is captured as `self.inner.out.len()` immediately before
delegating to the wrapped `TextSink`. That read of `inner.out` is the
only one `IndexingTextSink` makes; after this change it makes none.
Nothing else about the sink's structure changes — `raw_base`, the
`Mark` delegation, and the span-push order are all untouched.

### S2. `prototext-core/src/serialize/render_text/mod.rs`

Delete the five tests at `:917-1087` and the `// ── NodeSpan::
natural_annotation ──` section header at `:883`, together with any
fixture in that block (`:883-916`) left with no other user.

These tests assert the value of a field that will not exist. There is
nothing to preserve: they were written by spec 0122's test plan item 1
to prove the field was populated correctly, and correct population of a
field nobody reads is not a property worth keeping.

### S3. `protolens`

Delete the `natural_annotation: None` initializer at
`protolens/src/extract.rs:370`, `:413`, `:459`, and the eight in the
test fixtures (`tui/tests/support.rs`, `render.rs`, `prefetch.rs`,
`override_select.rs`, `key_dispatch.rs`).

Delete the stale doc comment at `tui/tests/override_apply.rs:199`
("panic on `natural_annotation`'s `.expect()`") — the `.expect()` it
describes was removed by spec 0135. The test itself stays; only the
sentence that misdescribes it goes.

### S4. Docs

- `docs/prototext/decode-flaws.md` P2 gets a resolution note.
- `docs/protolens/rendering-worklist.md` W25 step 5 (`:1448`) records
  that the step is discharged by this spec.
- `docs/protolens/rendering-scaling-roadmap.md`'s `NodeSpan` field
  table (`:682`) marks the row done.
- `docs/specs/0173`'s N5 (which deferred exactly this) is marked as
  taken up here.

Spec 0122 and spec 0135 are **not** edited. They are historical records
of what was decided when; the correction belongs in this spec (G3) and
in the flaws entry, not retro-fitted into the documents that got it
wrong.

## Test plan

- **The compiler finds every site.** Removing a `pub` struct field
  breaks every initializer, and all of them are in this repo. There is
  no way to miss one.
- **The existing suites must stay green, unchanged.** Nothing observable
  changes: no rendered byte, no span range, no line count. If a test
  other than the five deleted ones needs a change, something else
  changed too and the deletion is not as clean as this spec claims.
- **No new test.** The property this spec establishes is "this field
  does not exist", which the type system asserts and a test cannot.
- `reuse lint` and `nix-build -A ci` as usual.

## Note on measurement

The ~330 MB figure is arithmetic from committed measurements (`NodeSpan`
= 120 B, 0.566 nodes/byte, 24.5 MB fixture), not a fresh benchmark. It
is not load-bearing: the change is justified by the field having no
reader, and would be correct if it saved nothing. No benchmark run is
required to accept this spec, and none should be cited as if one had
been done.
