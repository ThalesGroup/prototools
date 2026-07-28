<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0201 — a hash inside a string is not a comment

Status: draft
App: protolens (and reproto, which links the same grammar)
Refs: docs/specs/0196-the-textproto-grammars-lexer-loses-tokens.md (the
        previous pass over the same forty lines of `grammar.js`, and the
        `roles_across` test helper this spec reuses),
      docs/specs/0116-tree-sitter-textproto-highlight-captures.md (the
        capture set)

## Background

Reported on 2026-07-28, against a rendered `FileDescriptorProto` payload:

```
4: "\n\006Metric\0229\n\nfirst_seen\030\001 \001(\0132\032.google.protobuf.TimestampR\tfirstSeen…
```

Everything from the first `#` in the payload to the end of the line
colors as `@comment`. In a quoted string a `#` is just another
character; nothing may turn it into a comment.

### It is not an escaping problem

`comment` is declared an `extra` (`grammar.js:300-303`), and tree-sitter
offers every extra as a candidate token in **every** lex state. A string
is not one token — `double_string` is

```js
seq('"', repeat(choice($.string_escape, $.double_string_contents)), '"')
```

— so there is a lex state *between* its inner tokens, and `comment` is
valid there like everywhere else. At the `#`,
`double_string_contents` (`/[^\n"\\]+/`) can only reach the next
backslash, while `comment` (`seq('#', /.*/)`) reaches end of line.
Neither carries precedence, so longest match decides and the comment
wins.

Reproduced against a locally regenerated parser, on the reported line
reduced to its shape:

```
4: "\022#\n\rexport_x"  #@ bytes = 4

(double_string [0,3]-[1,4]
  (string_escape          [0,4]-[0,8])    "\022"
  (comment                [0,8]-[0,36])   "#\n\rexport_x"  #@ bytes = 4
  (double_string_contents [1,0]-[1,3]))
```

The string never closes, so it swallows the following line as well.

That the `#` here directly follows an escape is why the defect looks
intermittent rather than universal: when the `#` sits in the middle of an
ordinary run of characters, the enclosing `double_string_contents` token
has already started and there is no lex state boundary at the `#` for the
comment to be offered at.

### The same boundary loses whitespace

The same lex state boundary means a space that starts a contents run is
skipped as a whitespace `extra` instead of being taken into the string.
`b: "p\n q"` yields contents `q`, not ` q`. Invisible today because
`@string` has no background, but it is the same defect.

## Goals

- **G1.** Inside a string, `#` is an ordinary character — a string
  containing one still terminates at its own closing quote, and a real
  trailing annotation comment on the same line still colors as
  `@comment`.
- **G2.** Every byte between a string's quotes belongs to the string,
  whitespace included.

## Non-goals

- **N1.** Un-vendoring `grammar.js`. Spec 0196 N1's reasons stand and
  this spec adds a seventh local fix to them.
- **N2.** Removing `comment` from `extras`. See A2.
- **N3.** Anything about `#@` annotation *content* — this spec is about
  where a comment may begin, not what one means.

## Specification

### S1. The string body outranks the extras

```js
single_string_contents: $ => token(prec(1, /[^\n'\\]+/)),
double_string_contents: $ => token(prec(1, /[^\n"\\]+/)),
string_escape:          $ => token(prec(1, choice( … ))),
```

tree-sitter's lexer compares explicit token precedence **before** match
length (spec 0196 Background 2 is the same mechanism, used against us
there). Raising the three string-body tokens to precedence 1 makes each
of them beat `comment` and `/\s/` on the one bidding contest that
matters, and only there: `single_string_contents`,
`double_string_contents` and `string_escape` are valid in no state
outside a string body, so nothing else in the grammar can be affected.

The closing quote needs nothing: no string-body token can match `"`.

## Alternatives considered

### A1. `token.immediate` on the string body

The obvious reading of "no extras in here", and what this spec's author
first assumed. **Rejected — it was built and it does not work.**
Immediacy forbids a token from being preceded by a skipped extra; it does
not withdraw `comment` from the candidate set at that position. The
regenerated parser still produced the `comment` node inside the string.
It did shorten the damage — the string now ends at the newline with a
`MISSING """` instead of running on — which is exactly the sort of
partial improvement that would have been mistaken for a fix.

### A2. Take `comment` out of `extras` and place it explicitly

Correct in principle: a comment may appear between fields, not inside a
token sequence. Rejected as far too invasive for the defect — every
position where a comment is legal would have to be enumerated in an LR
grammar, and getting the list wrong makes ordinary documents stop
parsing.

### A3. Make the whole string one `token(...)`

`double_string: $ => token(seq('"', repeat(…), '"'))` admits no extras at
all, by construction. Rejected: `token()` requires terminal contents, so
the `string_escape` children disappear and escapes lose the distinct
`@string.escape` color that spec 0196 spent its S1/S2 securing.

## Test plan

1. `4: "\022#\n\rexport_x"  #@ bytes = 4` — the `#` inside the string
   carries `StringLiteral` and no `Comment`; the trailing `#@ bytes = 4`
   is a `Comment`.
2. A `#` in an ordinary run (`s: "trail#"  # real`) and a `#` in a single
   -quoted string (`s: 'has # hash'`) — same two assertions.
3. `a: "x" "y"` still parses as two concatenated strings, so raising the
   precedence has not broken the whitespace between them.
4. `b: "p\n q"` — the space after the escape is `StringLiteral`, not
   dropped (G2).
5. The `test/highlight/textproto.txt` corpus grows a line with a `#`
   inside a string and a real comment after it, since `tree-sitter test`
   is what CI runs (`treeSitterTextprotoHighlightTest`).
6. Spec 0196's escape tests still pass unchanged — precedence 1 must not
   disturb the escape/contents contest `token()` already settled.

## Measured outcome

(to be filled in)
