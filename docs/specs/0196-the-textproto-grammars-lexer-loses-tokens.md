<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0196 — the textproto grammar's lexer loses tokens

Status: implemented
Implemented in: 2026-07-27
App: protolens (and reproto, which links the same grammar)
Refs: docs/specs/0116-tree-sitter-textproto-highlight-captures.md (the
        capture set and the `colorize.rs` pipeline),
      docs/specs/0121-tree-sitter-textproto-field-no-vendoring.md (why
        `grammar.js` is vendored and modified, and the `frac_digits`/
        `exp` precedence patch this spec partly reverts)

## Background

Three coloring defects reported on 2026-07-27, against real rendered
payloads. All three are real, all three are in
`reproto/tree-sitter-textproto/grammar.js`, and none of them is in
`highlights.scm` or `colorize.rs`. Reviewing the grammar to confirm them
turned up four more.

Every claim below was reproduced against a locally regenerated parser
(`tree-sitter generate` from the pinned CLI, linked into `colorize.rs`
through `TREE_SITTER_TEXTPROTO_LIB_DIR`), and the parse trees quoted are
that parser's own `to_sexp()` output.

### 1. `\\` is not an escape at all

The reported line:

```
2: "\027\002\\"  #@ string
```

`string_escape` (`grammar.js:175-192`) lists ten alternatives. Two of
them are written

```js
      "\\\"",
      ...
      '\\"',
```

which in JavaScript are **the same two characters** — backslash,
quote. So the list contains `\"` twice and `\\` **zero times**.

A `\\` in the payload therefore lexes as a lone `\` — recovered as an
octal escape with a missing digit — followed by `\"`, which eats the
string's own closing quote:

```
(double_string (string_escape (oct)) (double_string_contents)
               (string_escape (oct)) (double_string_contents)
               (string_escape (MISSING oct)) (string_escape)
               (double_string_contents) (MISSING """))
```

The string never terminates, so `  #@ string` is inside it and colors as
`@string` rather than `@comment`. That is exactly the report.

### 2. The exponent token eats the first letter of an identifier

The reported line:

```
end: 6  #@ int32 = 2
```

`end` is not being mistaken for a float. It is being **lexed** as one.
Spec 0121's fix for the `48.8566` defect wrote

```js
exp: $ => seq(
  token(prec(2, /[Ee][-+]?/)),
  /\d+/,
),
```

The leading `[Ee][-+]?` is its own token and carries lexer precedence 2.
In tree-sitter's lexer, explicit token precedence is compared **before**
match length — so wherever `exp` is in the valid-token set, a one-
character `e` beats a three-character `identifier`. After spec 0121's
`field_no` addition merged the relevant LALR states, `exp` is valid
immediately after a top-level scalar number, which is where field names
live:

```
"start: 1\nend: 6\n"
  → (field …(dec_int)) (ERROR) (field (field_name (identifier)) …)
     "start" Attribute   "nd" Attribute
```

The `e` becomes a stray `ERROR` and `nd` becomes the field name — which
is precisely "the leading `e` is uncolored, `nd` is light blue".

It is not specific to `end`, and not specific to floats: `Email` colors
as `mail`, and the report's own document would have shown the same on
any `e`- or `E`-initial field name following a scalar number. The worst
case swallows a line boundary —

```
"x: 1.5\ne: 6\n"  → (number (float (float_lit (dec_int)
                       (ERROR (frac_digits)) (frac_digits))))
```

— one `number` node spanning both lines.

### 3. An octal or hex escape keeps only its first digit

The reported line:

```
location: "\n\004\004\000hello"  #@ bytes
```

Not idiomatic — a bug, and the same shape as 1 and 2. The octal escape is

```js
seq("\\", $.oct, optional($.oct), optional($.oct))
```

with `oct: /[0-7]/` — **four separate tokens**. After `\0`, the lexer may
take either `oct` (one character) or `double_string_contents`
(`/[^\n"\\]+/`, greedy, two characters). Neither carries precedence, so
longest-match decides and the trailing octal digits are never taken:

```
"\\0" StringEscape
"04"  StringLiteral
```

`\x41` splits the same way, into `\x4` + `1`. `\uXXXX` does **not**,
because all four of its hex digits are required — with no optional tail,
`double_string_contents` is not in the valid set at that point and the
question never arises. That asymmetry is the tell: the defect is the
optional trailing digits, not the escapes.

### 4. `\U010…` is one hex digit short

`grammar.js:191` is `seq("\\U010", $.hex, $.hex, $.hex, $.hex)` — a
`\U` escape of **seven** hex digits. protobuf's `\U` takes eight. The
intent is clear from its sibling (`\U000` + five digits covers
`U+00000`–`U+FFFFF`), so the second arm should be `\U0010` + four,
covering `U+100000`–`U+10FFFF`. As written, the entire top plane fails
to parse:

```
"s: \"\\U0010FFFF\"\n"
  → (double_string (ERROR (identifier)))
```

### 5. `message` is defined twice, and the winner rejects an empty document

`grammar.js:20` and `grammar.js:41` both define `message` — first as
`repeat($.field)`, then as `repeat1($.field)`. It is one JavaScript
object literal, so the second silently wins and the first is dead code.

`message` is also the **start rule** (it is first). With `repeat1`, an
empty document does not parse:

```
""                 → (ERROR)
"\n"               → (ERROR)
"# just a comment" → (ERROR (comment))
```

### 6. A float's `f` suffix never attaches

`1.5f` parses as `float_lit` plus a sibling `ERROR`, so only `1.5`
colors as a number. `colorize.rs:279-289`'s `float_still_number` records
this as a known upstream limitation and asserts the truncated span.

The cause is that `/[Ff]/` and `identifier` both match `f` at the same
length in the same state. It is fixable — `token.immediate` says the
suffix may not be preceded by whitespace, which is what distinguishes a
float suffix from the next field's name, and no other rule needs to
change.

### 7. Two dead declarations

```js
precedences: $ => [
  ["message_list", "scalar_list"],
],
```

declares a relative order between two **named** precedences. Neither
name is ever used: `message_list` and `scalar_list` use numeric
`prec(2)`/`prec(1)`. The block has no effect.

`oct` and `hex` (`grammar.js:194-195`) exist only to be referenced from
`string_escape`, and S2 removes those references.

### 8. Not a defect, but worth recording

`nan` colors as `@constant` while `inf` and `-inf` color as `@number`.
`highlights.scm:54-57` lists the two `inf` spellings and not `nan`.
protobuf accepts all three, and the grammar cannot tell any of them from
an enum value name without a schema, so this is a one-line list, not a
grammar question. See N4.

## Goals

- **G1.** `\\` is an escape, and a string containing one terminates.
- **G2.** An escape's whole text is one `string_escape` node —
  `\004` colors as one escape, not `\0` plus two ordinary characters.
- **G3.** An identifier is never truncated by a number token, wherever
  it appears.
- **G4.** Every escape protobuf's text format accepts parses, and
  `\U0010FFFF` is one of them.
- **G5.** An empty document, and a comment-only document, parse without
  error.
- **G6.** A float's `f`/`F` suffix is part of the float.
- **G7.** No declaration in `grammar.js` is dead.

## Non-goals

- **N1.** Un-vendoring. Spec 0121's reasons stand, and this spec adds
  more: six of the seven findings are upstream's — only Background 2 is
  ours — so tracking upstream would reintroduce them.
- **N2.** Validating escapes semantically. `\400` is out of range for a
  byte and will still parse as an octal escape. The grammar's job is to
  delimit a token, not to range-check it.
- **N3.** New captures. The capture set is spec 0116's and does not
  change; this spec makes the existing captures land on the right bytes.
- **N4.** `nan` (Background 8). It is a `highlights.scm` predicate list
  and not part of the lexer story this spec is about; it is recorded so
  the next reader does not have to rediscover it.
- **N5.** Tightening `type_name`/`domain`, which today accept `a..b` and
  a trailing dot. Both are upstream's, neither can occur in protolens's
  own rendered output, and both color correctly when they do occur.

## Specification

### S1. `\\` joins the escape list, and `\"` stops being in it twice

The character-escape alternatives collapse to one character class,
which is how the duplicate went unseen:

```js
seq("\\", /[abfnrtv?'"\\]/),
```

### S2. Every escape is a single lexer token

The whole rule becomes one `token(...)`, so the lexer commits to an
entire escape or to none of it and `double_string_contents` never gets
to bid for its digits:

```js
string_escape: $ => token(choice(
  seq("\\", /[abfnrtv?'"\\]/),
  seq("\\", /[0-7]/, optional(/[0-7]/), optional(/[0-7]/)),
  seq("\\x", /[0-9A-Fa-f]/, optional(/[0-9A-Fa-f]/)),
  seq("\\u", /[0-9A-Fa-f]{4}/),
  seq("\\U000", /[0-9A-Fa-f]{5}/),
  seq("\\U0010", /[0-9A-Fa-f]{4}/),
)),
```

`token()` requires its contents to be terminal, so `$.oct` and `$.hex`
are inlined as the character classes they were, and the two rules are
deleted (G7). Nothing outside `string_escape` referenced them —
`highlights.scm` captures `(string_escape)` as a whole, and
`split_fdps.py` walks only `field`, `message` and `message_value`.

This is also G4's fix: `\U0010` replaces `\U010`.

### S3. The exponent is a single token that requires its digits

```js
exp: $ => token(prec(2, /[Ee][-+]?[0-9]+/)),
```

The precedence stays — spec 0121 needed it and the reason has not gone
away — but it now attaches to a token that **cannot match** unless
digits follow. `end` no longer offers the lexer anything to prefer over
`identifier`, and `1e5`, `1043E-04` and `48.8566e2` are unaffected.

`frac_digits` keeps its `token(prec(2, /\d+/))` unchanged. It was
checked against `dec_int`, `oct_int`, `hex_int` and `field_no` in the
states where all of them are valid, and steals from none of them: a
digit run cannot begin an identifier, so the collision that S3 fixes has
no counterpart here.

### S4. The start rule is separate from the message body

```js
document: $ => repeat($.field),
...
message: $ => repeat1($.field),
```

The duplicate key goes away and each of the two uses gets the rule it
wants: a document may be empty, a **message body** may not — `message`
must stay `repeat1` because `message_value` already wraps it in
`optional()`, and a nullable rule inside an `optional` is ambiguous.

The root node's type changes from `message` to `document`. Nothing reads
it: `colorize.rs` consumes highlight events rather than node types, and
`split_fdps.py` uses `root_node` directly and only ever compares its
*children's* types.

### S5. The float suffix is immediate

```js
float: $ => choice(
  seq($.float_lit, optional(token.immediate(/[Ff]/))),
  seq($.dec_int, token.immediate(/[Ff]/)),
),
```

`token.immediate` matches only with no preceding whitespace, which is
exactly what separates `1.5f` from `1.5` followed by a field named `f`.
`colorize.rs`'s `float_still_number` loses its "known limitation" note
and asserts the whole literal.

### S6. The dead `precedences` block is removed

Nothing references either name (Background 7). Removing it is not a
behavior change; leaving it would leave the next reader looking for the
`prec("scalar_list", ...)` that does not exist.

## Alternatives considered

### A1. Give the escapes' inner tokens a precedence instead of S2

The smaller edit: leave `string_escape` a sequence of nodes and put
`prec` on `oct`/`hex` so they outbid `double_string_contents`. Rejected
— it is precedence-against-length again, which is the mechanism that
produced Background 2, and it would have to be right in three places
instead of one. `token()` states the actual intent: an escape is one
lexical unit.

The cost is the loss of the `(oct)`/`(hex)` child nodes, which is
acceptable because nothing consumes them and `highlights.scm` colors the
escape as a whole.

### A2. Drop the `prec` from `exp` entirely rather than S3's rewrite

Tempting, since Background 2 is a precedence bug. Rejected: spec 0121
measured that without it `48.8566` loses its fraction digits, and that
regression is at least as visible as this one. Requiring the digits in
the same token keeps 0121's fix and removes its side effect, rather than
trading one defect for the other.

### A3. Make `message` nullable and keep one rule

The obvious way to read Background 5 — one rule, `repeat`, used in both
places. Rejected: `message_value` is `seq(open, optional($.message),
close)`, and a nullable rule under `optional` is an ambiguity
tree-sitter will reject or resolve arbitrarily. Two rules is the honest
encoding of two different requirements.

### A4. Fix only the three reported defects

Defensible on scope. Rejected because findings 4-7 are the same lexer
story as 1-3, they are found by reading the same forty lines, and each
one costs a `tree-sitter generate` and a Nix rebuild to land separately.

## Test plan

All in `colorize.rs`'s test module, which already exercises the grammar
end-to-end through `colorize()`.

1. `s: "\027\002\\"  #@ string` — the string terminates, the `\\` is one
   `StringEscape`, and the trailing `#@ string` is a `Comment`.
2. `\004`, `\x41` and `\400` each color as a single `StringEscape`
   covering the whole escape, with no `StringLiteral` fragment inside.
3. `\uXXXX`, `\U000XXXXX` and `\U0010FFFF` each color as one
   `StringEscape` — the third of these does not parse today.
4. All ten character escapes, `\a` through `\"` and `\\`, in one string.
5. `start: 1` followed by `end: 6` colors `end` — the whole word — as
   `Attribute`, and there is no `ERROR` node. Same for `Email`, and for
   a scalar number of each kind (`dec_int`, `float`, `oct_int`,
   `hex_int`) on the preceding line.
6. `x: 1.5` followed by `e: 6` produces two fields, not one `number`
   spanning the newline.
7. `48.8566`, `48.8566e2` and `1043E-04` still color whole — spec 0121's
   tests, unchanged and still passing.
8. `1.5f` and `1043E-04f` color whole, including the suffix; `1.5 f` (a
   space before it) does not, and yields a second field.
9. An empty document, a whitespace-only document and a comment-only
   document parse without an `ERROR` node, and the comment still colors
   as `Comment`.
10. The existing `test/highlight/textproto.txt` corpus grows the cases
    above so `tree-sitter test` covers them too, since that is what CI
    runs (`treeSitterTextprotoHighlightTest`).
11. `reproto`'s `split_fdps` tests still pass — the start rule's rename
    is only safe if nothing reads the root node's type.

## Measured outcome

All seven findings resolved. The three reported lines now color as the
report says they should:

| input | before | after |
|-------|--------|-------|
| `2: "\027\002\\"  #@ string` | string runs to end of line, `#@ string` colors as `@string` | string closes at the quote, `#@ string` is `@comment` |
| `end: 6  #@ int32 = 2` | `e` is a stray `ERROR`, `nd` is the field name | `end` is one `@attribute` |
| `"\n\004\004\000hello"` | `\0` is `@string.escape`, `04` is `@string` | each `\004` is one `@string.escape` |

And the four found while reviewing: `\U0010FFFF` parses (the whole top
Unicode plane was unreachable), `1.5f` and `1043E-04f` color whole, an
empty and a comment-only document parse without an `ERROR`, and the two
dead declarations are gone.

450 tests in `--bin protolens`, up from 445: four new `colorize` tests
(the backslash escape, an escape's optional digits, all eleven character
escapes plus the three `\u`/`\U` forms, and the truncated identifier
across six kinds of preceding number), one new test for the empty
document, and `float_still_number` renamed to
`float_suffix_joins_the_literal` — it now asserts the whole literal
including the suffix instead of documenting the truncation as a known
limitation.

`tree-sitter test`, which is what CI runs
(`treeSitterTextprotoHighlightTest`), goes from 77 to 99 assertions over
`test/highlight/textproto.txt`: five new fixture lines covering the two
escape defects, the three `\u`/`\U` widths, the float suffix, and an
`end: 6` placed deliberately after a float so it sits in the state the
exponent-token defect needed.

One test helper was added — `roles_across`, which reports every role
overlapping a needle rather than requiring an exact span match. The
escape defects are *span splits*, so `roles_at`'s exact-range comparison
returns an empty vector for the broken grammar and the fixed one alike,
and could not have caught them.
