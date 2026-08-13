<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0285 — a document token says what it is

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0282-a-wire-byte-says-what-it-is.md (the box this
        borrows whole, and `flaw_clause`, the table it takes over)
      docs/specs/0280-a-heat-cue-says-what-its-score-is-made-of.md
        (S6-S17 — the dwell, the anchor, the dismissal; S10, the one
        document-row span already spoken for)
      docs/specs/0283-the-reading-points-back-at-the-byte.md (S3/S4 —
        `BoxLine` and its mark)
      docs/specs/0225-the-wire-row-is-under-the-line-it-explains.md
        (S11/S12 — `crate::annotation`, the one place a keyword's
        severity is decided)
      docs/prototext/annotation-format.md (the grammar this lexes and
        the definitions this quotes)

## Background

The wire row explains itself token by token (0282, 0283). The document
row above it explains itself at exactly one span — the type name in the
`#@` annotation, which opens the score box (0280 S10) — and is silent
everywhere else.

The row is not self-evident. A reader looking at

```
int32Pk: -1  #@ repeated int32 [packed=true] = 85; pack_size: 5; neg
```

can point at seven things and get an answer for none of them: what `85`
counts, what `[packed=true]` changes, what `pack_size` is measuring,
what `neg` accuses. The answer for the last one is *already compiled
in* — `popup_wire.rs::flaw_clause` says "a negative value in five
bytes, not the canonical ten" — but only to a reader hovering the hex
underneath, which is the audience least likely to need it. Everyone
else opens `annotation-format.md`.

## Goals

- **G1.** A token on the document row says what it is.
- **G2.** One copy of every explanation. `neg` reads the same in the
  wire box and in the document box because there is one string.
- **G3.** The explanation costs more dwell than the datum. A reader who
  wants a glossary entry says so more heavily than one asking about
  their own bytes.
- **G4.** No new key, no new pane, and no parse per motion event.

## Non-goals

- **N1.** The type name is not an element. 0280 S10 owns that span and
  answers a better question about it — how well these bytes fit this
  type — than a glossary could. One span, one target, one dwell (S5).
- **N2.** The value's box does not decode anything. It says which part
  of the line this is and why it is spelled the way it is (S4); it does
  not re-read the bytes, because *what are these bytes* is a different
  question with a better answer already built — `w` and the wire box.
- **N3.** No keyboard route. 0280 S18's `s` opens the score box at the
  caret because the caret already names a node; the caret also names a
  column, so the same thing is possible here — but the box is a
  glossary, and a reader at the keyboard has `?` and the format
  reference. Not ruled out, just not bought.
- **N4.** The header line `#@ prototext: protoc` is not a target. It is
  a file format marker, drawn once, and says the same thing to every
  document.
- **N5.** Nothing is added to the wire box. `flaw_clause` moves house
  (S1) and gains entries (S2); what a wire hit prints is unchanged.

## Specification

- **S1.** The explanation strings move to `crate::annotation`, beside
  `tier_of`. That module is already "the single place that decides"
  how serious a keyword is (0225 S11); it now also decides what the
  keyword says, so severity and meaning cannot come apart.
  `popup_wire.rs::flaw_clause` becomes `annotation::clause`, entries
  unchanged, and `popup_wire.rs` calls it.

- **S2.** The table is completed. `flaw_clause` was written for what a
  wire row can accuse a *byte* of, so it is missing every keyword that
  only ever reaches the document row: `ohb`, `truncated_neg`,
  `MISSING`, and the informational `pack_size`. It also gains
  `packed_ohb` and `packed_truncated_neg`, which spec 0227 established
  are v1 residue that prototext no longer *emits* but still accepts —
  a hand-written file may carry one, protolens colors it, so it must
  be explainable. `vocabulary()`'s drift test gains "every keyword has
  a clause", so the next modifier prototext-core adds cannot arrive
  unexplained — the same guarantee that test already gives
  `highlights.scm`.

- **S3.** The row's tokens are found by lexing the row, not the tree.
  The annotation is a grammar we own:
  `#@ [wire_type; ][label ]type[ [packed=true]] = number[; modifier]*`,
  tokens separated by `"; "`, each modifier `name` or `name: value`.
  `popup.rs::annotation_type_span` already walks exactly this, so it is
  replaced by one pass producing every element of the row in ascending
  order; both queries over it — "which element is at this byte" and
  0280's "where is the type name" — are `find`s. Two parsers over one
  grammar is how they come to disagree. The lexer reads the row *as
  drawn*, so with `a` having hidden the annotations there is simply
  nothing right of the value to point at.

- **S4.** Eight elements, and what each says:

  | the pointer is on | the box says |
  |---|---|
  | the field key, left of `:` or `{` | which of the three kinds it is — a name the schema gave it, a bare number because no schema matched, or a bracketed extension |
  | the value, right of `: ` | the field's value, printed as `protoc --decode` prints it — and, where the row already says why, why it looks like that: a bare `0x…` because no schema said how to read those bits, a bare identifier because the schema has that name for the number the annotation carries |
  | `#@` | that it opens a prototext annotation, that the rest of the line is how the bytes were encoded rather than part of the message, and that prototext is textproto plus these annotations |
  | a wire-type token | which wire type it is and what is on the wire for it |
  | `repeated` / `required` | the label, and that `optional` is the omitted default |
  | `[packed=true]` | this field's elements share one length-prefixed record |
  | `= 85` | the field's number in its `.proto` message — what is on the wire, where the name is not |
  | a modifier keyword | the token as drawn, its tier if it has one, and `annotation::clause` |

  The `#@` row is the only one that names the format, and deliberately:
  the marker is the one element whose box is about the whole trailing
  part, so the term can be coined there without being repeated under
  every member of the annotation. Three things, and the box keeps them
  apart — `#@` is the *marker*, a **prototext annotation** is everything
  from it to end of line, and **prototext** is textproto plus those.
  `docs/prototext/annotation-format.md` opens with the same three, so
  the box is not naming something the reference does not.

  Every one of these is answered from the drawn row alone. The value's
  two "why" cases in particular look up nothing: a `0x…` is what the
  renderer writes when there is no declared type to read the bits as,
  and an identifier beside a `Name(7)` type token is that schema's name
  for 7 — both facts are already on the line, which is what keeps this
  a lexer and not a second reader of the blob (N2).

- **S5.** A point has exactly one target. The element lexer skips the
  type name's span, which N1 leaves to 0280. Without this the reader
  resting on a type is shown the score box at 400 ms and can never
  reach the longer dwell behind it.

- **S6.** A modifier's value is part of the target, not part of the
  clause. `tag_ohb: 3` is one token; the box prints it as drawn on its
  own line and the clause underneath, so the number the reader pointed
  at is in front of them without the clause having to be a format
  string with one caller.

- **S7.** `EXPLAIN_DWELL` is 900 ms, against `HOVER_DWELL`'s 400.
  0280's 400 ms sits at the low end of the desktop 400-600 ms range
  because the answer is already computed and the reader asked about one
  specific node. This is the other case: the pointer crossing a dense
  annotation on its way somewhere passes over five explainable tokens,
  and at 400 ms each crossing would open five boxes. 900 ms is past the
  top of that range — long enough to be the pause of someone who has
  stopped to read rather than someone travelling. It is one field
  either way: `hover_deadline` is set from the target rather than from
  a constant.

- **S8.** Everything else is 0280's, unchanged: the same `Popup` and
  `anchored_rect`, the same block on the menu, the override pane and
  the splash, the same "anything at all dismisses it", the same rule
  that only tearing down an open box owes a frame. `PopupBody` gains a
  third variant carrying `Vec<BoxLine>`; the box has no ranking to
  apply, so it needs nothing of 0282 S10's `fit`.

- **S9.** The code lives in a new `tui/popup_doc.rs`, mirroring
  `popup_wire.rs`: the hit test and the body builder, and nothing else.
  `popup.rs` keeps the timing and the chrome and stays ignorant of
  which body it is holding, exactly as its module doc already claims.

- **S10.** The help text's Display section gains the document-row hover
  beside the wire-row one it already carries.

## Alternatives considered

### Explain the type name too, behind the longer dwell

One span, two boxes, chosen by how long the pointer stayed: at 400 ms
the score, at 900 ms a glossary entry replacing it. A box that turns
into a different box while it is being read is worse than either box,
and it makes the dwell a mode the reader has to know about.

### Key the table by the drawn token

A map from the word to its explanation, which needs no lexer. It is
wrong because the same word appears twice on one row meaning two
things:

```
1000: "binary data"    #@ bytes          <- the wire type: a length-prefixed blob
bytesOp: "\000\001"    #@ bytes = 32     <- the proto type this field is declared with
```

`fixed32` and `fixed64` have the same double life. A word-keyed table
would give the wire-type answer in both places, and would also claim
the second one — which is the type name, and belongs to 0280 (N1).
Only the lexer knows which part of the annotation a token sits in.

### Reuse the highlighter's spans

The obvious source — `window_styles` already assigns every span a
`SyntaxRole`. 0280 S10 rejected it and the reasons stand: the hints
index into `display_row_text`, which has no fold margin; they are
cleared while `input_pending`; and asking for them per motion event
costs a parse (G4). They are also the wrong granularity —
`SyntaxRole::Attribute` is both the field key and the `= 85`.

### A glossary pane, or a `?`-style lookup

The question is "what is this thing under my pointer". A pane makes the
reader carry the token to the answer, and by then they have retyped it.

## Test plan

1. `every_keyword_has_a_clause` — S2, over `vocabulary()`, `pack_size`
   and the wire-type names; and `the_two_clause_tables_do_not_overlap`.
2. `a_keyword_reads_the_same_in_both_boxes` — G2, for `val_ohb`, on the
   one blob that draws it on both rows: the wire box's flaw line is the
   document box's clause with the keyword written in front of it.
3. `a_modifier_is_explained_with_its_value` — S6, over `val_ohb: 3`,
   both tiers, and `pack_size`, which carries neither.
4. `the_annotation_declaration_is_explained` — S4 for the label, the
   `[packed=true]` flag and the `= 85`, on one packed repeated row.
5. `a_wire_type_token_is_not_read_as_a_scalar_type` — the `bytes`
   collision, from both positions on one document.
6. `the_field_key_says_which_kind_it_is` — S4, over a schema name, a
   bare number and a bracketed extension.
7. `the_value_says_why_it_is_spelled_that_way` — S4/N2, over a plain
   scalar, an unschema'd `0x…` and an enum name.
8. `the_type_name_still_opens_the_score_box` — S5/N1.
9. `an_explanation_waits_longer_than_a_score` — S7: after a hover on a
   modifier, `hover_deadline` is further out than `HOVER_DWELL`; after
   one on a type name, it is not.
10. `a_document_box_is_dismissed_like_any_other` — S8, by a keystroke
    and by a click.
11. `the_help_text_documents_the_document_hover` — S10.
12. `a_keyword_with_no_clause_is_not_a_target` — N4, over the
    `#@ prototext: protoc` header and over a word the vocabulary does
    not carry.

## Measured outcome

Implemented as specified. Two things the drafting did not know:

- The type name's span had to be refused **twice**, and deliberately.
  `handle_hover` tries 0280's `type_annotation_at` first and falls back
  to this one, *and* `doc_element_at_point` refuses `DocElement::Type`
  on its own. Either alone would do; both together make the rejected
  "one span, two boxes" alternative unreachable rather than merely
  unwritten.
- N4 costs nothing. Refusing a modifier whose keyword has no clause —
  written so that a box reading only the word already under the pointer
  never opens — excludes the `#@ prototext: protoc` header for free.
  `every_keyword_has_a_clause` is what stops that arm from quietly
  swallowing the vocabulary instead.

Spec 0280's `hover_over_a_type_name_arms_the_dwell` asserted that the
tokens beside a type name armed *nothing*; four of its assertions now
read "does not name a type", which is what it was always testing.
