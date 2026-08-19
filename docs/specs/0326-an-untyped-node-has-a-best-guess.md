<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0326 — an untyped node has a best guess

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0280-a-heat-cue-says-what-its-score-is-made-of.md (the
        score box, its dwell, and its one hover target),
        docs/specs/0285-a-document-token-says-what-it-is.md (the
        document box, and the rule that one point has one target),
        docs/specs/0287-the-chrome-beside-a-row-says-what-it-is.md (the
        hover ordering this inserts into)

## Background

Spec 0280 gives the reader one hover target in the document pane: the
type name in a `#@` annotation. Point at it and the box says how badly
the bytes fit it.

A node with no schema behind it has no type name to point at. It draws

```
1 {  #@ message
```

and the only word in the type's place is `message` — which is not a type
name, is lexed as an unremarkable modifier, and has no clause, so
`doc_element_at_point` refuses it (0285 S4's last arm). Pointing at it
produces nothing at all. The heat cue in column 0 says a better type
exists and prints `[?/47]`, and the one gesture that would say *which*
type is not available on the one node where the question is most worth
asking.

The same is true of a group with no schema (`#@ group`), and half true
of a group with one (`#@ group; Foo = 3`), where `group` and `Foo` sit
two tokens apart and only the second is a target.

## Goals

- **G1.** `message` and `group`, standing where a type name would stand,
  are score-box anchors.
- **G2.** For a node with no type of its own, the box names the
  best-scoring candidate and breaks its score down — the same five terms
  0280 already prints, about a type the node could be rather than one it
  is.
- **G3.** The box says plainly that it is showing a candidate. A reader
  must not come away believing the node is already typed that way.
- **G4.** No sweep. The answer comes out of the cache the heat cue has
  already filled, or the box says it is not there yet.

## Non-goals

- **N1.** *Not a ranking.* Spec 0280 N1 stands: the override pane
  answers "what else could this be", and it lists every candidate with
  its score. This box answers "why that one" about a single type. The
  tie count (S5) is the one concession, and it is a count, not a list.
- **N2.** *No new dwell and no new box.* This is 0280's box, at 0280's
  `HOVER_DWELL`, opened from two more spans. A third body would be a
  third thing to keep consistent.
- **N3.** *`message` is not added to the wire-type vocabulary.*
  `annotation::wire_type_clause` names the five real wire types and
  `highlights.scm` carries the same five; `message` is what the
  *renderer* writes for an unknown LEN field that decoded, not a wire
  type. Adding it there would change the token's color as a side effect
  of a hover change, and would drag a tree-sitter rebuild into it.
- **N4.** *The candidate is not offered for acceptance.* No key in this
  box applies it. The reader who wants it has `t` and the override pane,
  which is where choosing a type lives.

## Specification

- **S1.** The score box's hover zone is every span of the drawn row for
  which `is_score_anchor` holds:
  - `DocElement::Type` — the declared type name, as today; or
  - a token whose text is exactly `message` or `group`.

  The second clause is by *text*, not by position: `group` lexes as
  `DocElement::WireType` and `message` as `DocElement::Modifier`, and no
  other member of the annotation vocabulary is spelled either word, so
  one predicate covers both without the caller having to know which
  element each landed in.

- **S2.** `is_score_anchor` is the single rule. `annotation_type_spans`
  filters by it and `doc_element_at_point` refuses by it, replacing
  0285's hard-coded `DocElement::Type` refusal. Two copies of this rule
  would come apart into a token that opens both boxes or neither.

  Consequence, accepted: a `group` keyword no longer opens the document
  box that explained *wire types 3 and 4*. It is one of five wire-type
  names and the only one that can also be a message, which is exactly
  why the reader points at it.

- **S3.** `open_score_popup` splits on `current_type_key(idx)`:
  - `Some(key)` — today's box, unchanged.
  - `None` — the **candidate** box of S4.

  The split is on the node, not on which span was hovered. A known
  group's `group` keyword therefore opens the ordinary box about the
  group's own type, which is what the reader pointing at
  `group; Foo = 3` is asking about either way.

- **S4.** The candidate box takes the top-ranked name out of
  `by_range`'s `top_n` — which `record_sweep` always writes at least one
  entry of — and runs `inferred_breakdown` against it: one type, one
  range, the same synchronous call 0280 N4 already justified. With no
  cache entry the box says `still scoring these bytes`, in
  `SuffixShape::Unknown`'s own words, which is what the `[?]` beside it
  is already saying.

- **S5.** The candidate box is titled `best candidate` in its border,
  and prints one extra line under the terms when others tie:
  `n others also score s`. `s` is the breakdown's own sum, not the
  cached rank score, so the two numbers in the box cannot disagree. The
  tie count is `best_count - 1` — *others*, since the named one is the
  first of them.

- **S6.** Spec 0287 S6's ordering is untouched: chrome is tested before
  the row is lexed, and within the row the score anchors are tried
  before the explanation. S1 only widens what "the score anchors" means.

## Alternatives considered

**A fourth `DocElement` for the framing keyword.** A `Framing` variant
holding `message`/`group` would name the thing instead of matching its
text. Rejected: it is a fifteenth member of an enum every arm of
`doc_lines` must answer for, to distinguish a case that one `matches!`
already distinguishes, and it would split `group`'s lexing between two
variants depending on whether a schema was found.

**Showing the whole ranking in the box.** The reader who wants the list
has the override pane, and it is a pane because a list does not fit in a
hover box. Spec 0280 N1 drew this line and nothing here moves it.

**Calling `sweep::ranked` on demand.** It is the honest way to answer
when the cache is cold, and it costs about 0.9 s on a real range — two
orders of magnitude past a 400 ms dwell. `top_n` is already there for
free and is the same ranking.

**Leaving `message` unlexed and hanging the anchor off the `{`.** The
brace is on every message row, typed or not, and would make the whole
line a target for one box or another. Spec 0280's "hover the type you
are asking about has exactly one edge" is the reason it is a word.

## Test plan

1. `message_and_group_are_score_anchors` — `annotation_type_spans` on
   `1 {  #@ message` yields the `message` span, on `1 {  #@ group` the
   `group` one, and on `g {  #@ group; Foo = 3` both, in that order. A
   real wire type (`#@ bytes`) yields none, so the widening did not
   swallow the four other wire-type names.
2. `a_score_anchor_is_not_a_document_box` — `doc_element_at_point` over
   the `message` and `group` columns returns `None`, so the two boxes
   cannot both open, and the hover over `message` lands on
   `HoverTarget::Type` rather than on nothing.
3. `an_untyped_node_shows_its_best_candidate` — a message node with
   `NO_FQDN` and a seeded `RangeHeatEntry`: the box's `type_key` is
   `top_n[0]`'s name, its `candidate` is `best_count - 1`, and a drawn
   frame carries the border title.
4. `only_a_candidate_box_counts_the_ties` — `candidate: Some(3)` prints
   `3 others also score s` with `s` the breakdown's own sum;
   `Some(0)` and `None` print no such line, the last being test-plan
   item 7's typed node.
5. `an_unscored_range_is_pending_not_unranked` — no cache entry gives
   `Breakdown::Pending` and the words `still scoring these bytes`.

## Measured outcome

Implemented 2026-08-19 as specified, in five tests rather than seven —
items 1 and 2 of the draft plan are one assertion set over one pure
function, and item 7 is the `candidate: None` arm of the tie test.

Two things the implementation settled that the spec had left open:

- **`Breakdown::Pending` is a fourth variant, not a reuse of
  `Unranked`.** The same distinction 0280 S4 already draws between
  `NoGraph` and `Unranked`: a verdict and the absence of one are
  different answers, and both would otherwise render as a box the
  reader cannot tell apart.
- **`score_breakdown` split into it and `breakdown_of(idx, key)`.** The
  memo is keyed on `(range.start, key)` and so already tells a
  candidate's breakdown from the current type's, which is why the split
  needed no second cache.

No column arithmetic moved and no existing test changed except to name
the new `candidate` field.
