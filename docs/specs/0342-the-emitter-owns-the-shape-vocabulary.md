<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0342 — the emitter owns the shape vocabulary

Status: implemented
Implemented in: 2026-08-20
App: prototext-core
Refs: docs/specs/0341-a-shape-name-is-a-type-name.md (the two-name gap
        this closes the cause of; its N1 and S4 survive unchanged),
      docs/specs/0227-the-vocabulary-has-one-home-and-the-encoder-refuses-
        what-it-cannot-encode.md (draft, unimplemented — its S1 proposed
        publishing the same list as `pub const [&str; N]` and argued
        against threading it through the emit sites; this spec takes the
        seven-name half and reverses that argument, for the reason under
        Alternatives),
      docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (S11/
        S12 — the vocabulary and its drift test, whose input this
        rewires),
      docs/prototext/annotation-format.md (the format's own list, which
        stays prose)

## Background

Spec 0341 fixed a two-name gap between what the renderer emits and what
the highlighter colors. It did not fix the reason the gap could open.

The seven shape names are string literals at six call sites
(`sink.rs:559`, `:577`, `:659`, `:721`, `:755`, `varint.rs:176`) plus
two `ScalarCtx` initializers (`sink.rs:483`, `:534`). Nothing in
prototext-core is the list. A seventh name was added to the emitter at
some point by writing a new literal, and no build, test or lint could
notice that four downstream copies had not heard about it.

The drift test is the sharpest illustration.
`every_keyword_is_colored_by_its_tier` (`colorize.rs`) exists precisely
to stop `highlights.scm` falling behind, and it did not fire — because
its input is `annotation::vocabulary()`, a hand-written list in
protolens, and the query and that list agreed with each other. The test
compares two mirrors. Neither of them is the thing.

## Goals

- **G1.** The shape vocabulary exists as one value in prototext-core,
  and the emitter's call sites name its members. An eighth shape cannot
  be emitted without being declared.
- **G2.** protolens's `vocabulary()` derives from that value, so the
  drift test's input is the emitter's list rather than a copy of it.
  `highlights.scm` is then checked against the emitter, transitively,
  by the test that already exists.

## Non-goals

- **N1.** **The encoder is not converted.**
  `encode_text/fields.rs:234` matches `varint`, `fixed64`, `fixed32` and
  `bytes` and lets everything else fall through to inference from the
  value's own shape — which is how `string` and `message` are read back,
  correctly, today. Making that match exhaustive over `Shape` would
  force two arms whose bodies are "do what the fall-through already
  does". The encoder is a partial reader of the vocabulary on purpose.
- **N2.** `annotation::wire_type_clause` still answers for five names
  only, and `WIRE_TYPE_NAMES` still holds those five, for the reasons in
  spec 0341 N1: `string` and `message` are not wire types, and the hover
  box lexes them as modifiers (spec 0326 N3).
- **N3.** **`highlights.scm` stays hand-maintained.** A tree-sitter
  query is data in another language, generated into a Nix store path; it
  cannot import a Rust const. What changes is that the test which checks
  it now reads the emitter instead of a sibling copy — the query is
  still written by hand, but it can no longer be wrong in silence.
- **N4.** The `INVALID_*` names do not become an enum. They are sixteen
  strings that the encoder must also parse, they carry no ordering and
  no exhaustiveness obligation, and the one place that would benefit —
  `push_invalid` — has a single caller.
- **N5.** `anomaly_fixture.rs`'s `VOCABULARY` stays a literal list, per
  spec 0341 S4. It is an assertion about *that fixture*, not about the
  renderer, and deriving it from `Shape::ALL` would make it wrong.

## Specification

- **S1.** New `prototext-core/src/serialize/render_text/shape.rs`
  defining

  ```rust
  pub enum Shape { Varint, Fixed64, Fixed32, Bytes, String, Message, Group }
  ```

  with `Shape::ALL` and `const fn as_str(self) -> &'static str`.
  Re-exported as `prototext_core::Shape`, following the module's
  existing `mod x; pub use x::Y;` pattern.

  A **shape** is what the renderer decided a record is when no schema
  said. Five of the seven are wire types; three of them — `bytes`,
  `string`, `message` — are the readings wire type 2 admits, in the
  order the unknown-LEN cascade tries them.

- **S2.** `AnnWriter::push_wire` splits in two:
  `push_shape(&mut self, out, shape: Shape)` and
  `push_invalid(&mut self, out, name: &str)`. One door per vocabulary.
  The split is what makes S1 binding: while a single `&str` door served
  both, the parameter could not be typed, because `render_invalid`
  (`helpers/scalar.rs:116`) passes an `INVALID_*` name through it.

- **S3.** `ScalarCtx::wire_type_name: &'a str` becomes
  `shape: Shape`, and the struct loses its lifetime-carrying reason to
  hold a name it never inspects.

- **S4.** The group header (`sink.rs:755`) uses `push_shape(Shape::Group)`
  rather than `push(out, b"group")`. It was the one shape written
  through the raw-token door, which is why it never appeared in a grep
  for the others.

- **S5.** protolens's `annotation::LEN_SHAPE_NAMES` is deleted and
  `vocabulary()` maps `Shape::ALL` through `as_str`. `WIRE_TYPE_NAMES`
  stays at five (N2) and gains a test asserting every member is a
  `Shape` — the tie that makes the two lists unable to disagree about
  spelling.

## Alternatives considered

**A `pub const SHAPE_NAMES: [&str; 7]` instead of an enum — spec
0227 S1's proposal, for the whole 30-token vocabulary.** Smaller, and it
would give `vocabulary()` something real to read. 0227 argued explicitly
against threading such a const back through the emit sites: "substituting
a constant into a format string does not make the two agree — it only
moves the copy", and leaned instead on the fixture's set-equality test to
keep the published list honest.

That argument holds for the 23 anomaly tokens, which is why N4 leaves
them alone. It does not hold for the seven shapes, and 0341 is the
counterexample: the fixture *cannot* exhibit `string` or `message` (it
reaches every anomaly through a declared field), so the set-equality test
was structurally blind to precisely the two names that drifted. With no
test able to see them, a published const would document the list without
owning it, and an eighth shape could still be emitted as a bare literal
with the const left behind — the failure of 0341, one indirection later.

The seven shapes differ from the anomaly tokens in the two ways that
matter: they are closed and few, and they are written at eight sites that
are all plain `push` calls rather than format strings. So the enum is
affordable here and is the point — the emitter cannot spell a shape it
has not declared, which is a guarantee no test has to be clever enough to
notice.

**Move the tiers and clauses into prototext-core too, so the whole
annotation vocabulary has one home.** Rejected: a tier is a display
decision — it exists to pick a color — and a clause is protolens's
prose. prototext-core has no theme and no hover box, and the two crates
would then share a module that only one of them can explain. What
belongs in the emitter is what the emitter emits.

## Test plan

1. `every_keyword_is_colored_by_its_tier` (`colorize.rs`) — unchanged
   test, input now derived from `Shape::ALL`. This is G2: the query is
   now checked against the emitter.
2. `the_wire_type_names_are_shapes` (`annotation.rs`) — new. Every
   member of `WIRE_TYPE_NAMES` is a `Shape`, so the two lists cannot
   disagree about spelling even though they deliberately differ in
   length.
3. `the_fixture_covers_the_whole_vocabulary` (`anomaly_fixture.rs`) —
   unchanged, and must stay green: it is the check that the *rendered
   text* is byte-identical, which is what a refactor of the emitter has
   to prove.

## Measured outcome

Implemented 2026-08-20. Eight literal sites became variants; no rendered
byte changed, which is what `the_fixture_round_trips_byte_exact` and
`the_fixture_covers_the_whole_vocabulary` assert. Workspace suite 1563
tests, green with and without `COLORTERM`.

Two things the implementation established that the plan did not predict:

- **`Shape::Group` was written through a different door.** The group
  header called `push(out, b"group")`, the raw-token function, not
  `push_wire`. A grep for the other six names could not find it, and a
  const-only fix (the rejected alternative) would have had no reason to
  touch it. It is the clearest evidence that the vocabulary needed a
  type rather than a list.

- **The two lists that stay separate now cost one test, not a comment.**
  `WIRE_TYPE_NAMES` deliberately holds five of the seven (N2), and that
  is exactly the shape of divergence nothing notices. It is now checked
  as a subset by spelling, so the pair can differ in length — which is
  intended — without being able to differ in wording, which is not.
