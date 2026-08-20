;;; SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
;;;
;;; SPDX-License-Identifier: MIT

; Base captures (unchanged from upstream — kept as-is per spec 0116
; Non-goals: existing 5 captures preserved).
(string) @string
(field_name) @attribute
(comment) @comment
(number) @number
(open_squiggly) @punctuation.bracket
(close_squiggly) @punctuation.bracket
(open_square) @punctuation.bracket
(close_square) @punctuation.bracket
(open_arrow) @punctuation.bracket
(close_arrow) @punctuation.bracket

; §1/§2 — extension_name/any_name, both reachable only through
; field_name (field_name: $ => choice(extension_name, any_name,
; identifier) has no other tokens of its own), so the blanket
; (field_name) @attribute above already covers their byte ranges and
; these patterns only narrow parts of them by last-match-wins.
;
; An extension name is left @attribute deliberately: `[acme.blade]: 3`
; names a field, and on the wire it is a tag like any other field's, so
; it belongs in the field-name color rather than in the type color it
; used to borrow.
;
; An Any key does hold a type name, and that half takes @type. The
; domain half is a URL, not a type, and keeps its own capture.
(any_name
  (domain) @string.special.url
  (type_name) @type)

; §3 — string_escape gets its own capture, sibling to the enclosing
; string's @string (both fire on overlapping ranges by design).
(string_escape) @string.escape

; §4 — bare identifier scalar values. Blanket @constant declared first
; (schema-blind default: without a .proto schema, the grammar cannot
; tell an enum value name (KNOWN_ENUM_VALUE) from a bool/inf value
; handled below, or from a genuinely invalid/unknown scalar. @constant
; is chosen as the least-wrong default: enum-value-name identifiers are
; by far the more common case for a non-true/false/inf bare identifier
; in practice, and @constant is the closest standard capture
; semantically ("a named, unchanging value"). Declared first so the
; more specific patterns below can override it (tree-sitter's
; last-match-wins precedence is declaration order, not predicate
; specificity).
(scalar_value (identifier) @constant)
(scalar_value (signed_identifier) @constant)

((scalar_value (identifier) @boolean)
 (#any-of? @boolean "true" "false"))

; Narrowed from the historical unconditional @number pattern — @number
; now applies only to the inf/-inf identifier values it was actually
; meant for. Declared last (most specific) so it wins over both
; patterns above for these two exact values.
((scalar_value (identifier) @number)
 (#eq? @number "inf"))
((scalar_value (signed_identifier) @number)
 (#eq? @number "-inf"))

; §5 — delimiter punctuation.
[":" "," ";"] @punctuation.delimiter

; §6 — split @punctuation.bracket by context. Message body braces/
; angle-brackets are already covered by the blanket patterns above,
; unchanged (the "default" bracket kind). Square brackets are a single
; pair of named token rules reused across four different contexts
; (message_list, scalar_list, extension_name, any_name) — disambiguate
; through the parent node. These context-scoped patterns are declared
; after the blanket (open_square)/(close_square) @punctuation.bracket
; patterns above so the more specific .list/.extension captures win by
; last-match-wins declaration order.
(message_list (open_square) @punctuation.bracket.list)
(message_list (close_square) @punctuation.bracket.list)
(scalar_list (open_square) @punctuation.bracket.list)
(scalar_list (close_square) @punctuation.bracket.list)

(extension_name (open_square) @punctuation.bracket.extension)
(extension_name (close_square) @punctuation.bracket.extension)
(any_name (open_square) @punctuation.bracket.extension)
(any_name (close_square) @punctuation.bracket.extension)

; §7 (spec 0225 S12) — protolens's own `#@` annotations. The grammar
; gives them structure (see `annotation` in grammar.js) but stays
; keyword-blind; the vocabulary lives here, mirroring
; protolens/src/annotation.rs, because a query change needs no
; `tree-sitter generate`. protolens's
; `the_annotation_vocabulary_matches_the_grammars_captures` test fails
; if the two lists drift apart.
;
; Declared first: an annotation is still a comment, and every pattern
; below narrows a part of it. Anything the narrower patterns do not
; claim — the `#@` marker, the `;` separators, junk — stays comment
; gray, which is the right default for punctuation and for a keyword
; nobody has classified yet.
(annotation) @comment

(annotation_number) @number

; An enum field's wire value, `Color(99)` or the packed
; `Color([1, 2])`. A number, wearing the number color, exactly like the
; symbolic name it stands for on the document half of the line. The
; parentheses go with it: they are one token, and a third split to make
; them punctuation would buy nothing the eye needs.
(annotation_enum_value) @number

; The declaration item — `[label] type [enum_value] [attribute] "="
; number`, e.g. `repeated int32 [packed=true] = 85` or `Type(5) = 5`.
;
; The label and the type both take @type: they are the two halves of
; one statement about what this field is, and together they are what
; the wire row's wire type borrows (spec 0225 S11), so splitting
; their colors would make that half of the tag depend on whether the
; field happened to be repeated.
((annotation_word) @type
 (#any-of? @type "repeated" "required"))

; A shape name fills the same slot when there is no schema — an
; unknown or invalid field has nothing else to say what it is — so it
; takes the same color, and the wire row's wire type stays typed rather
; than going gray exactly where the bytes matter most. The INVALID_*
; names are not here: they are anomalies and the tiers below claim
; them.
;
; Seven names, not five: `bytes`, `string` and `message` are the three
; readings the renderer can give wire type 2, and they occupy one slot
; and answer one question, so they take one color (spec 0341).
((annotation_word) @type
 (#any-of? @type
  "varint" "fixed64" "fixed32" "bytes" "group" "string" "message"))

; The type name itself, identified by what follows it rather than by a
; list the query would have to keep: a word immediately before an `=`,
; an enum value, or a `[packed=true]` is the declared type.
((annotation_item (annotation_word) @type . (annotation_eq)))
((annotation_item (annotation_word) @type . (annotation_enum_value)))
((annotation_item (annotation_word) @type . (annotation_attribute)))

; `[packed=true]` itself has no rule, and falls through to the comment
; color (spec 0267 S1, amended). It is not a *third* thing the field is:
; the label and the type name say what the field is, and packing says
; how the encoder chose to write it — the same kind of fact
; `pack_size: N` states two tokens later, in the same color.

; The field number, after the `=`. It is the document's field number,
; and the field-name color is what says so — the same color the name it
; belongs to wears at the head of the line, and the same the wire row's
; tag bytes borrow.
((annotation_item (annotation_eq) . (annotation_number) @attribute))

; The severity tiers, last so they win over @type for a keyword that
; happens to sit before an `=`. Shape names (varint, fixed64, fixed32,
; bytes, string, message, group) are deliberately absent: they are the
; document's own vocabulary quoted back, not an anomaly, and the @type
; pattern above is what claims them. `pack_size` is absent for the same
; reason (spec 0267 S2): it counts a record's elements, which is a fact
; about the encoding and not a complaint about it, so it falls through
; to the comment color every unclassified token takes.
((annotation_word) @annotation.non_canonical
 (#any-of? @annotation.non_canonical
  "tag_ohb" "val_ohb" "len_ohb" "etag_ohb" "ohb" "packed_ohb"
  "nan_bits" "neg" "truncated_neg" "packed_truncated_neg"
  "ENUM_UNKNOWN"))

((annotation_word) @annotation.invalid
 (#any-of? @annotation.invalid
  "TAG_OOR" "ETAG_OOR" "TYPE_MISMATCH" "MISSING" "END_MISMATCH"
  "OPEN_GROUP" "INVALID_TAG_TYPE" "INVALID_VARINT" "INVALID_FIXED64"
  "INVALID_FIXED32" "INVALID_LEN" "INVALID_GROUP_END"
  "INVALID_STRING" "INVALID_PACKED_RECORDS"
  "TRUNCATED_BYTES" "TRUNCATED_MESSAGE"))
