<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0341 — a shape name is a type name

Status: implemented
Implemented in: 2026-08-20
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (S11/
        S12 — the annotation vocabulary, its `highlights.scm` mirror and
        the drift test that is supposed to keep the two together; this
        corrects the mirror and the drift test's input),
      docs/specs/0285-a-document-token-says-what-it-is.md (S4 —
        `wire_type_clause`, which is left at five names on purpose),
      docs/specs/0326-an-untyped-node-has-a-best-guess.md (N3 — the
        hover box lexes `message` as a modifier; unchanged),
      docs/prototext/annotation-format.md (the format's own list, wrong
        in the same two places)

## Background

On `grpconf/stage/boblog` the root row reads

```
#@ message
```

in comment gray, while the row under a `varint` field reads `#@ varint`
in the type color. `#@ string` on nodes `/1/2` and `/1/4/2` is gray for
the same reason. All three tokens sit in the same slot and answer the
same question — what protolens decided this field is, absent a schema —
so two of them going gray is not a distinction the reader can act on.

The cause is a two-name gap between the emitter and the highlighter.
`AnnWriter::push_wire`
(`prototext-core/src/serialize/render_text/helpers/annotations.rs:148`)
is the single writer of a schema-blind shape name, and across its call
sites it emits **seven**: `varint`, `fixed64`, `fixed32`, `bytes`,
`group`, `string` (`sink.rs:559`) and `message` (`sink.rs:721`).
`highlights.scm`'s `#any-of?` list named five. The two missing names
therefore fell through to the first-declared `(annotation) @comment`,
which is the correct default for an unclassified token and the wrong
answer for these two.

Neither could be rescued by the positional rules that claim a declared
type name, because all three require the word to be immediately followed
by an `=`, an enum value or a `[packed=true]`, and a schema-blind
annotation is the bare word with nothing after it.

The five-name list turns out to exist in **four** places: the query, the
drift test's input, `annotation-format.md`'s `valid_wire_type`
production, and `anomaly_fixture.rs`'s `VOCABULARY`. All four omitted
the same two names.

**The drift test could not see this.** `every_keyword_is_colored_by_its_tier`
(`colorize.rs`) exists precisely to stop `highlights.scm` falling behind
`annotation.rs`, but its only input is `annotation::vocabulary()`, whose
untiered half was `WIRE_TYPE_NAMES` — the same five names. The test and
the query agreed with each other and both disagreed with the emitter. A
third copy, `annotation-format.md`'s `valid_wire_type` production, had
the identical omission.

## Goals

- **G1.** All seven names `push_wire` can emit take the type color.
- **G2.** The drift test's input covers all seven, so the next name
  added to the emitter cannot repeat this in silence.

## Non-goals

- **N1.** `annotation::wire_type_clause` is **not** extended. It answers
  "wire type *N* — …", and neither `string` nor `message` is a wire type:
  both are wire type 2, already spoken for by `bytes`. It is also the
  recognizer `popup_doc.rs` uses to lex a token as
  `DocElement::WireType`, and spec 0326 N3 depends on `message` falling
  through to `DocElement::Modifier` there. The hover box is unchanged;
  only the color moves.
- **N2.** No rename of `DocElement::WireType`, `SyntaxRole` or spec
  0285's vocabulary to match the word "shape" used here. The concept is
  named correctly in the two documents that define it
  (`annotation-format.md`, this spec); renaming the code would touch
  four modules to say the same thing.
- **N3.** The gap is not closed *by construction*. `push_wire` takes a
  `&str`, and its `render_invalid` call site passes an `INVALID_*` name
  through the same door, so an enum parameter would have to cover the
  anomaly vocabulary too. The list stays hand-maintained; what changes
  is that it now lives in one place the drift test reads.

## Specification

- **S1.** `highlights.scm`'s shape `#any-of?` list gains `"string"` and
  `"message"`.
- **S2.** `annotation::LEN_SHAPE_NAMES` is a new test-only const holding
  those two, and `vocabulary()` chains it after `WIRE_TYPE_NAMES` with
  tier `None`. It is a second const rather than two more entries in the
  first because `WIRE_TYPE_NAMES` is also `wire_type_clause`'s domain
  (N1), and the two tests that iterate it assert a clause exists for
  every member.
- **S3.** `annotation-format.md`'s Part 1 table and its
  `valid_wire_type` production list all seven, with the three LEN
  readings distinguished by what the payload turned out to admit.
- **S4.** `anomaly_fixture.rs`'s `VOCABULARY` stays at five and says why
  in its doc comment. Its assertion is an equality in both directions,
  and the fixture reaches every anomaly through a declared field of
  `FileDescriptorProto`, so no LEN payload there arrives untyped: listing
  the two names would fail as `missing`. What was wrong was the comment
  claiming the list is what the renderer can emit.

## Alternatives considered

**Fold the two names into `WIRE_TYPE_NAMES`.** One const instead of two,
but it drags `wire_type_clause` along: `every_keyword_has_a_clause`
requires a clause for every member, so `string` and `message` would have
to answer there — and answering there makes them `DocElement::WireType`
in the hover box, which contradicts spec 0326 N3 and changes behavior the
user did not ask about.

**Give `message` and `string` a color of their own,** distinct from both
the type color and the comment color, on the grounds that they are
inferences rather than declarations. Rejected: the inference is already
visible — a declared type is spelled `Foo = 3`, a shape is a bare word —
and a third color would say twice what the shape of the token says once.

**Claim them positionally, as the declared type name is claimed.** There
is no position to claim: a schema-blind annotation can be the single
token `#@ message`, with neither a neighbor nor an `=` to key on.

## Test plan

1. `every_keyword_is_colored_by_its_tier` — unchanged test, widened
   input. Now asserts `string` and `message` take the type color, which
   is what G2 buys.
2. `a_backslash_escape_does_not_swallow_the_closing_quote` — the fossil.
   Its assertion that `#@ string` is wholly comment-colored, and the
   comment explaining why, recorded the defect; both are corrected to
   `[Comment, Type]` — the marker gray, the shape name typed.
3. `test/highlight/textproto.txt` — two `tree-sitter test` assertions,
   one on `#@ string` and one on `#@ message`, so the query is pinned
   where it is written as well as where it is consumed.

## Measured outcome

Implemented 2026-08-20. `tree-sitter test` 126 → 128 assertions; the
protolens suite 1203 tests, unchanged and green.

`highlights.scm` is read through `include_str!` of the Nix store path
`build.rs` forwards, so a query edit takes a rebuild of
`treeSitterTextprotoRustLib` and a fresh `nix-shell` before any Rust test
can see it.

The `group` question this came from, recorded because the answer is not
obvious from either file: a group's message name appears twice and is
colored differently on purpose. In the annotation — `#@ group; Foo = 3`
— `Foo` precedes an `=` and takes the type color, beside the `group`
keyword this spec's list claims. In the document — `Foo {` — it sits in
`message_field`'s `field_name` slot (`grammar.js:53`) and takes
`@attribute`, the field-name color, because that is what the slot means.
Protobuf groups spell the field name as the message name; the type-ness
of it is stated in the annotation, and there it is typed.
